//! Keeps the game's own bytes for everything a mod doesn't change.
//!
//! The merger rebuilds a whole pack when a mod touches one file in it: every file inside goes through its parser and
//! comes out compressed and laid out again. The content is the same, the bytes aren't, and a re-serialized file is one
//! more thing that can go wrong on a game version the parsers weren't written for. After a merge, each file inside a
//! rebuilt pack that still says what the player's own game file says gets the game's bytes back.

use std::path::Path;

use anyhow_ext::{Context, Result};
use fs_err as fs;
use roead::{
    aamp::ParameterIO,
    byml::Byml,
    sarc::{Sarc, SarcWriter},
    yaz0::{compress, decompress_if},
};

use uk_content::{message::Msyt, resource::MergeableResource};

use crate::savage_wind::GameFiles;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Restored {
    /// Files inside packs that were already the game's bytes.
    pub same: usize,
    /// Files with the game's content in other bytes, now the game's bytes again.
    pub restored: usize,
    /// Files a mod really changes or adds.
    pub modded: Vec<String>,
    /// Packs written again here: the path in the merged folder, whether it is the DLC's, and the decompressed size
    /// now and in the game. The ResourceSizeTable entry has to cover the new size.
    pub rewritten: Vec<(String, bool, usize, usize)>,
}

fn is_sarc(data: &[u8]) -> bool {
    data.len() >= 0x14 && &data[..4] == b"SARC"
}

fn is_byml(data: &[u8]) -> bool {
    data.len() >= 4 && (&data[..2] == b"BY" || &data[..2] == b"YB")
}

fn is_aamp(data: &[u8]) -> bool {
    data.len() >= 4 && &data[..4] == b"AAMP"
}

/// Whether two files say the same thing: the same bytes once decompressed, the same archive members whatever the
/// order and padding, the same parameters or texts, or the same thing to the merger. The last one matters most: the
/// merger writes an AI program with its entries renumbered and an animation list with other string types, and a
/// file it reads the same way before and after is one no mod changed.
pub fn same_content(name: &str, a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let (a, b) = (decompress_if(a), decompress_if(b));
    if a == b {
        return true;
    }
    if is_sarc(&a) && is_sarc(&b) {
        let (Ok(a), Ok(b)) = (Sarc::new(a.as_ref()), Sarc::new(b.as_ref())) else {
            return false;
        };
        return a.len() == b.len()
            && a.files().all(|file| {
                file.name()
                    .and_then(|name| b.get_data(name))
                    .is_some_and(|other| same_content(file.unwrap_name(), file.data(), other))
            });
    }
    if is_byml(&a) && is_byml(&b) {
        return matches!((Byml::from_binary(&a), Byml::from_binary(&b)), (Ok(a), Ok(b)) if a == b)
            || same_to_the_merger(name, &a, &b);
    }
    if is_aamp(&a) && is_aamp(&b) {
        return matches!(
            (ParameterIO::from_binary(&a), ParameterIO::from_binary(&b)),
            (Ok(a), Ok(b)) if a == b
        ) || same_to_the_merger(name, &a, &b);
    }
    if name.ends_with(".msbt") {
        return matches!((Msyt::from_msbt_bytes(&a), Msyt::from_msbt_bytes(&b)), (Ok(a), Ok(b)) if a == b);
    }
    same_to_the_merger(name, &a, &b)
}

fn same_to_the_merger(name: &str, a: &[u8], b: &[u8]) -> bool {
    let name = Path::new(name);
    matches!(
        (MergeableResource::from_binary(name, a), MergeableResource::from_binary(name, b)),
        (Ok(Some(a)), Ok(Some(b))) if a == b
    )
}

/// Rebuilds `built` (a decompressed SARC) with the game's bytes for every member that matches `stock`'s. None when
/// nothing needed it.
fn restore_sarc(built: &[u8], stock: &[u8], prefix: &str, out: &mut Restored) -> Option<Vec<u8>> {
    let built_sarc = Sarc::new(built).ok()?;
    let stock_sarc = Sarc::new(stock).ok()?;
    let mut writer = SarcWriter::from_sarc(&built_sarc);
    let mut changed = false;
    for file in built_sarc.files() {
        let Some(name) = file.name() else { continue };
        let full = format!("{prefix}{name}");
        let Some(game) = stock_sarc.get_data(name) else {
            out.modded.push(full);
            continue;
        };
        if file.data() == game {
            out.same += 1;
        } else if same_content(name, file.data(), game) {
            writer.files.insert(name.into(), game.to_vec());
            out.restored += 1;
            changed = true;
        } else {
            let (inner, inner_game) = (decompress_if(file.data()), decompress_if(game));
            if is_sarc(&inner) && is_sarc(&inner_game) {
                if let Some(data) = restore_sarc(&inner, &inner_game, &format!("{full}//"), out) {
                    let data = if file.data().starts_with(b"Yaz0") { compress(data) } else { data };
                    writer.files.insert(name.into(), data);
                    changed = true;
                }
            } else {
                out.modded.push(full);
            }
        }
    }
    changed.then(|| writer.to_binary())
}

fn restore_file(path: &Path, game: &Path, name: &str, dlc: bool, out: &mut Restored) -> Result<()> {
    let built = fs::read(path)?;
    let stock = fs::read(game)?;
    if built == stock {
        return Ok(());
    }
    let (inner, inner_game) = (decompress_if(&built), decompress_if(&stock));
    if !is_sarc(&inner) || !is_sarc(&inner_game) {
        return Ok(());
    }
    if let Some(data) = restore_sarc(&inner, &inner_game, &format!("{name}//"), out) {
        out.rewritten.push((name.to_string(), dlc, data.len(), inner_game.len()));
        let data = if built.starts_with(b"Yaz0") { compress(data) } else { data };
        fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))?;
    }
    Ok(())
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(root, &path, files),
            Ok(_) => {
                if let Ok(rel) = path.strip_prefix(root) {
                    files.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
            Err(_) => {}
        }
    }
}

/// Runs over the merged mods (`content` and `aoc` are the merged folders for the base game and the DLC) after a
/// merge. `only` limits it to the files that merge rebuilt; None checks everything.
pub fn restore_merged(
    content: &Path,
    aoc: &Path,
    game: &GameFiles,
    only: Option<&dyn Fn(&str, bool) -> bool>,
) -> Restored {
    let mut out = Restored::default();
    for (root, dlc) in [(content, false), (aoc, true)] {
        let mut files = Vec::new();
        walk(root, root, &mut files);
        for file in files {
            if only.is_some_and(|only| !only(&file, dlc)) {
                continue;
            }
            let rel = file.strip_prefix("Aoc/0010/").unwrap_or(&file);
            let game_file = if dlc {
                game.all_aoc.iter().rev().map(|a| a.join(rel)).find(|p| p.is_file())
            } else {
                Some(game.romfs.join(rel)).filter(|p| p.is_file())
            };
            let Some(game_file) = game_file else { continue };
            if let Err(e) = restore_file(&root.join(&file), &game_file, &file, dlc, &mut out) {
                log::warn!("Couldn't compare {file} with the game's own: {e:?}");
            }
        }
    }
    log::info!(
        "Packs the mods touch: {} files inside kept the game's bytes, {} got them back, {} are modded",
        out.same,
        out.restored,
        out.modded.len()
    );
    for name in &out.modded {
        log::debug!("Modded: {name}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use roead::Endian;

    fn sarc(files: &[(&str, &[u8])], alignment: usize) -> Vec<u8> {
        let mut writer = SarcWriter::new(Endian::Little).with_min_alignment(alignment);
        for (name, data) in files {
            writer.add_file(*name, data.to_vec());
        }
        writer.to_binary()
    }

    #[test]
    fn same_content_ignores_packaging() {
        let text = b"0123456789abcdef0123456789abcdef0123456789abcdef".to_vec();
        assert!(same_content("a.bin", &compress(&text), &text));
        assert!(!same_content("a.bin", &compress(&text), b"other"));
        let a = sarc(&[("a.bin", &text), ("b.bin", b"bee")], 4);
        let b = sarc(&[("b.bin", b"bee"), ("a.bin", &compress(&text))], 0x100);
        assert_ne!(a, b);
        assert!(same_content("p.pack", &a, &b));
        assert!(!same_content("p.pack", &a, &sarc(&[("a.bin", &text)], 4)));
    }

    #[test]
    fn restore_keeps_only_the_mod_changes() {
        let text = b"0123456789abcdef0123456789abcdef0123456789abcdef".to_vec();
        let packed = compress(&text);
        let nested_game = sarc(&[("x.bin", &packed), ("y.bin", b"why")], 4);
        let game = sarc(&[("keep.sbin", &packed), ("edit.bin", b"old"), ("in.pack", &nested_game)], 4);
        // What a merger makes: the same content written out again, one real change at each level and a new file.
        let nested_built = sarc(&[("x.bin", &text), ("y.bin", b"WHY")], 4);
        let built = sarc(
            &[("keep.sbin", &text), ("edit.bin", b"new"), ("in.pack", &nested_built), ("added.bin", b"+")],
            4,
        );
        let mut out = Restored::default();
        let fixed_bytes = restore_sarc(&built, &game, "P//", &mut out).expect("something to restore");
        let fixed = Sarc::new(fixed_bytes.as_slice()).unwrap();
        assert_eq!(fixed.get_data("keep.sbin").unwrap(), packed.as_slice());
        assert_eq!(fixed.get_data("edit.bin").unwrap(), b"new");
        assert_eq!(fixed.get_data("added.bin").unwrap(), b"+");
        let nested = Sarc::new(fixed.get_data("in.pack").unwrap()).unwrap();
        assert_eq!(nested.get_data("x.bin").unwrap(), packed.as_slice());
        assert_eq!(nested.get_data("y.bin").unwrap(), b"WHY");
        assert_eq!(out.restored, 2);
        let mut modded = out.modded.clone();
        modded.sort();
        assert_eq!(modded, ["P//added.bin", "P//edit.bin", "P//in.pack//y.bin"]);
        // A second pass has nothing left to do.
        assert!(restore_sarc(&fixed_bytes, &game, "P//", &mut Restored::default()).is_none());
    }
}
