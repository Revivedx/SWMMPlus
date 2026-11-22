//! Wii U mods on the Switch game.
//!
//! Most of a mod's data files only differ in byte order, and the merger reads either. What it can't read across are
//! the files it treats as opaque: layout images, layouts, layout animations, fonts and texture files. Those are
//! converted here, in the mod's own files, before the mod is packaged. What can't be converted (models, sounds,
//! physics) is left out, so the game keeps its own, and noted for the player.

pub mod font;
pub mod layout;
pub mod mips;
pub mod model;
pub mod tex;

use std::path::Path;

use anyhow_ext::Result;
use fs_err as fs;
use parking_lot::Mutex;
use roead::{
    sarc::{Sarc, SarcWriter},
    yaz0::{compress, decompress_if},
};

/// What a conversion did, for the log and the player.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub textures: usize,
    pub layouts: usize,
    pub animations: usize,
    pub fonts: usize,
    /// Textures in texture files (icons, model textures).
    pub model_textures: usize,
    /// Files left out of the mod and why.
    pub left_out: Vec<(String, String)>,
}

impl Report {
    pub fn converted(&self) -> usize {
        self.textures + self.layouts + self.animations + self.fonts + self.model_textures
    }

    /// A few lines a player can read.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.converted() > 0 {
            lines.push(format!(
                "Converted from Wii U: {} images, {} layouts, {} layout animations, {} fonts, {} textures",
                self.textures, self.layouts, self.animations, self.fonts, self.model_textures
            ));
        }
        if !self.left_out.is_empty() {
            lines.push(format!(
                "{} Wii U files can't be converted and were left out (the game keeps its own):",
                self.left_out.len()
            ));
            let mut reasons: Vec<(&str, usize, &str)> = Vec::new();
            for (file, reason) in &self.left_out {
                match reasons.iter_mut().find(|r| r.0 == reason) {
                    Some(entry) => entry.1 += 1,
                    None => reasons.push((reason, 1, file)),
                }
            }
            for (reason, count, example) in reasons {
                lines.push(format!("  {count} x {reason} (for example {example})"));
            }
        }
        lines
    }
}

static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What the conversions since the last call have to tell the player.
pub fn take_notes() -> Vec<String> {
    std::mem::take(&mut *NOTES.lock())
}

enum Outcome {
    Keep,
    Replace(Vec<u8>),
    LeaveOut(String),
    /// Taken out without a word: the Switch has nothing like it and loses nothing.
    Drop,
}

fn wii_u_bom(data: &[u8], at: usize) -> bool {
    data.get(at..at + 2) == Some(&[0xFE, 0xFF])
}

/// One file that isn't an archive.
fn leaf(name: &str, data: &[u8], game: Option<&[u8]>, report: &mut Report) -> Outcome {
    let converted = |result: Result<Vec<u8>>, what: &str, count: &mut usize| match result {
        Ok(data) => {
            *count += 1;
            Outcome::Replace(data)
        }
        Err(e) => Outcome::LeaveOut(format!("{what} that couldn't be converted: {e}")),
    };
    let magic = data.get(..4).unwrap_or_default();
    match magic {
        b"FLYT" if wii_u_bom(data, 4) => converted(layout::convert_bflyt(data), "a layout", &mut report.layouts),
        b"FLAN" if wii_u_bom(data, 4) => {
            converted(layout::convert_bflan(data), "a layout animation", &mut report.animations)
        }
        b"FFNT" if wii_u_bom(data, 4) => converted(font::convert_bffnt(data), "a font", &mut report.fonts),
        b"FRES" if model::is_wii_u(data) && model::is_second_texture_file(name) => Outcome::Drop,
        b"FRES" if model::only_textures(data) => match model::convert_texture_file(data, game) {
            Ok((data, count)) => {
                report.model_textures += count;
                Outcome::Replace(data)
            }
            Err(e) => Outcome::LeaveOut(format!("a texture file that couldn't be converted: {e}")),
        },
        // The Switch's model files start "FRES" and four spaces.
        b"FRES" if data.get(4..8) != Some(b"    ") => {
            Outcome::LeaveOut("Wii U models and animations, which have another format on Switch".into())
        }
        b"FSTM" | b"FWAV" | b"FSTP" | b"FSAR" | b"FWAR" | b"FGRP" if wii_u_bom(data, 4) => {
            Outcome::LeaveOut("Wii U sounds".into())
        }
        b"BARS" if wii_u_bom(data, 8) => Outcome::LeaveOut("Wii U sounds".into()),
        // Havok files say their byte order in byte 0x11.
        [0x57, 0xE0, 0xE0, 0x57] if data.get(0x11) == Some(&0) => {
            Outcome::LeaveOut("Wii U physics and navigation data".into())
        }
        _ if name.ends_with(".bflim") => Outcome::LeaveOut("a layout image outside a layout archive".into()),
        _ => Outcome::Keep,
    }
}

/// An archive: its members one by one, and its layout images gathered into the Switch's texture archive.
fn archive(name: &str, data: &[u8], report: &mut Report) -> Option<Vec<u8>> {
    let sarc = Sarc::new(data).ok()?;
    let mut writer = SarcWriter::from_sarc(&sarc);
    if name.ends_with("arc") {
        writer.set_legacy_mode(true);
    }
    let mut changed = false;
    let mut textures = Vec::new();
    for file in sarc.files() {
        let Some(member) = file.name() else { continue };
        let full = format!("{name}//{member}");
        let outcome = if member.ends_with(".bflim") {
            let short = member.rsplit('/').next().unwrap_or(member);
            match tex::read_bflim(short, file.data()) {
                Ok(texture) => {
                    textures.push(texture);
                    report.textures += 1;
                    writer.files.shift_remove(member);
                    changed = true;
                    continue;
                }
                Err(e) => Outcome::LeaveOut(format!("a layout image that couldn't be converted: {e}")),
            }
        } else {
            file_outcome(&full, file.data(), None, report)
        };
        match outcome {
            Outcome::Keep => (),
            Outcome::Replace(data) => {
                match model::switch_name(member).filter(|_| data_is_switch_model(&data)) {
                    Some(renamed) => {
                        writer.files.shift_remove(member);
                        writer.files.insert(renamed.into(), data);
                    }
                    None => {
                        writer.files.insert(member.into(), data);
                    }
                }
                changed = true;
            }
            Outcome::Drop => {
                writer.files.shift_remove(member);
                changed = true;
            }
            Outcome::LeaveOut(reason) => {
                writer.files.shift_remove(member);
                report.left_out.push((full, reason));
                changed = true;
            }
        }
    }
    if !textures.is_empty() {
        // Only the mod's textures: the merger adds them to the game's atlas texture by texture. An atlas already
        // in a Wii U mod's archive is the game's own, put there when the mod was unpacked.
        textures.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
        match tex::build_bntx("__Combined", &textures) {
            Ok(bntx) => {
                writer.files.insert("timg/__Combined.bntx".into(), bntx);
            }
            Err(e) => {
                report.textures -= textures.len();
                report
                    .left_out
                    .push((format!("{name}//timg"), format!("layout images that couldn't be packed: {e}")));
            }
        }
    }
    changed.then(|| writer.to_binary())
}

fn data_is_switch_model(raw: &[u8]) -> bool {
    decompress_if(raw).starts_with(b"FRES    ")
}

/// `game` is the game's own file of this name, unpacked.
fn file_outcome(name: &str, raw: &[u8], game: Option<&[u8]>, report: &mut Report) -> Outcome {
    let data = decompress_if(raw);
    let packed = |new: Vec<u8>| if raw.starts_with(b"Yaz0") { compress(new) } else { new };
    if data.len() >= 0x14 && &data[..4] == b"SARC" {
        match archive(name, &data, report) {
            Some(new) => Outcome::Replace(packed(new)),
            None => Outcome::Keep,
        }
    } else {
        match leaf(name, &data, game, report) {
            Outcome::Replace(new) => Outcome::Replace(packed(new)),
            other => other,
        }
    }
}

fn walk(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => walk(&path, files),
            Ok(_) => files.push(path),
            Err(_) => {}
        }
    }
}

/// Whether an unpacked mod (or one of its options) is laid out as a Wii U mod: `content` and `aoc` folders.
pub fn is_wii_u_tree(root: &Path) -> bool {
    let here = |dir: &Path| dir.join("content").is_dir() || dir.join("aoc").is_dir();
    here(root)
        || std::fs::read_dir(root.join("options"))
            .map(|options| options.filter_map(|e| e.ok()).any(|e| here(&e.path())))
            .unwrap_or(false)
}

/// Converts what can be converted in an unpacked Wii U mod, in place, and leaves out what can't.
///
/// `game` are the folders of the game's own files (the base game's and the DLC's), for what a conversion completes
/// from them.
pub fn convert_tree(root: &Path, game: &[std::path::PathBuf]) -> Result<Report> {
    crate::origin::is_wii_u();
    let mut report = Report::default();
    let mut roots = vec![root.to_path_buf()];
    if let Ok(options) = std::fs::read_dir(root.join("options")) {
        roots.extend(options.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for mod_root in roots {
        for top in ["content", "aoc"] {
            let mut files = Vec::new();
            walk(&mod_root.join(top), &mut files);
            for path in files {
                let name = path.strip_prefix(&mod_root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                let raw = fs::read(&path)?;
                // The game's file of the same name: `content/A/B` and `aoc/0010/A/B` are both `A/B` there.
                let inner = name.strip_prefix("content/").or_else(|| name.strip_prefix("aoc/0010/"));
                let theirs = inner.and_then(model::switch_name).and_then(|inner| {
                    let raw = game.iter().find_map(|dir| std::fs::read(dir.join(&inner)).ok())?;
                    Some(decompress_if(&raw).to_vec())
                });
                match file_outcome(&name, &raw, theirs.as_deref(), &mut report) {
                    Outcome::Keep => (),
                    Outcome::Replace(data) => match model::switch_name(&name).filter(|_| data_is_switch_model(&data)) {
                        Some(renamed) => {
                            fs::remove_file(&path)?;
                            fs::write(mod_root.join(renamed), data)?;
                        }
                        None => fs::write(&path, data)?,
                    },
                    Outcome::Drop => fs::remove_file(&path)?,
                    Outcome::LeaveOut(reason) => {
                        fs::remove_file(&path)?;
                        report.left_out.push((name, reason));
                    }
                }
            }
        }
    }
    for line in report.lines() {
        log::info!("{line}");
    }
    for (file, reason) in &report.left_out {
        log::debug!("Left out {file}: {reason}");
    }
    NOTES.lock().extend(report.lines());
    Ok(report)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use roead::Endian;

    fn bflim(width: u16, height: u16) -> Vec<u8> {
        // An 8-bit alpha image: stored 128 wide and a multiple of 16 tall on the Wii U.
        let stored = 128 * (height as usize).div_ceil(16) * 16;
        let mut d: Vec<u8> = (0..stored).map(|i| i as u8).collect();
        d.extend_from_slice(b"FLIM\xFE\xFF\x00\x14\x05\x02\x00\x00");
        d.extend_from_slice(&((stored + 0x28) as u32).to_be_bytes());
        d.extend_from_slice(&[0, 1, 0, 0]);
        d.extend_from_slice(b"imag\x00\x00\x00\x10");
        d.extend_from_slice(&width.to_be_bytes());
        d.extend_from_slice(&height.to_be_bytes());
        d.extend_from_slice(&[0x08, 0x00, 0x01, 0x04]);
        d.extend_from_slice(&(stored as u32).to_be_bytes());
        d
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_layout_archive_gets_an_atlas_and_loses_what_cannot_convert() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let layout_archive = SarcWriter::new(Endian::Big)
            .with_legacy_mode(true)
            .with_file("timg/Key^d.bflim", bflim(48, 48))
            .with_file("timg/Bad.bflim", b"nothing".to_vec())
            .with_file("blyt/Broken.bflyt", b"FLYT\xFE\xFF\x00\x14garbage".to_vec())
            .with_file("blyt/Switch.bflyt", b"FLYT\xFF\xFE\x14\x00kept".to_vec())
            .to_binary();
        let pack = SarcWriter::new(Endian::Big)
            .with_file("Layout/Common.sblarc", compress(layout_archive))
            .with_file("Model/Thing.sbfres", compress(b"FRES\x03\x05\x00\x04\xFE\xFF".to_vec()))
            .with_file("Actor/Thing.bxml", b"AAMP data".to_vec())
            .to_binary();
        let option = root.join("options/A/content/Pack");
        fs::create_dir_all(&option).unwrap();
        fs::write(option.join("Bootup.pack"), &pack).unwrap();
        fs::create_dir_all(root.join("content/Model")).unwrap();
        fs::write(root.join("content/Model/Loose.sbfres"), compress(b"FRES\x03\x05\x00\x04".to_vec())).unwrap();
        assert!(is_wii_u_tree(root));

        let report = convert_tree(root, &[]).unwrap();
        assert_eq!((report.textures, report.layouts), (1, 0));
        assert_eq!(report.left_out.len(), 4);
        assert!(!root.join("content/Model/Loose.sbfres").exists());
        let pack = fs::read(option.join("Bootup.pack")).unwrap();
        let pack = Sarc::new(pack).unwrap();
        assert!(pack.get_data("Model/Thing.sbfres").is_none());
        assert_eq!(pack.get_data("Actor/Thing.bxml").unwrap(), b"AAMP data");
        let layout_archive = decompress_if(pack.get_data("Layout/Common.sblarc").unwrap()).to_vec();
        let layout_archive = Sarc::new(layout_archive).unwrap();
        let names: Vec<String> = layout_archive.files().filter_map(|f| f.name().map(String::from)).collect();
        assert!(!names.iter().any(|n| n.ends_with(".bflim") || n == "blyt/Broken.bflyt"), "{names:?}");
        assert!(names.iter().any(|n| n == "blyt/Switch.bflyt"));
        let atlas = uk_content::layout::bntx::Bntx::parse(layout_archive.get_data("timg/__Combined.bntx").unwrap())
            .unwrap();
        assert_eq!(atlas.textures.len(), 1);
        assert_eq!(atlas.textures[0].name, "Key^d");
        // Nothing left to do the second time.
        assert_eq!(convert_tree(root, &[]).unwrap(), Report::default());
    }
}
