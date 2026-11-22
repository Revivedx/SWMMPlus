//! Where each installed mod came from, and the list of deployed mods Savage Wind copies into its log.
//!
//! A mod is stored packaged whatever it came as, so what it came as (a BNP, whole game files, a Wii U mod) is
//! noted when it is converted, in `origins.json` beside the profiles. At each deploy, `savagewind-mods.txt` is
//! written at the root of the overlay: a first line starting with `#` that names SWMM+ and the profile, then one
//! line for each enabled mod, the one that wins first. Savage Wind copies the lines into `savagewind.log`, so a
//! bug report says which mods were loaded.

use std::{collections::BTreeMap, path::Path};

use fs_err as fs;
use parking_lot::Mutex;

use crate::mods::Mod;

pub const LIST_NAME: &str = "savagewind-mods.txt";
const ORIGINS: &str = "origins.json";

/// What the conversion under way has seen: what the mod came as, and whether it is a Wii U mod.
static SEEN: Mutex<(Option<&'static str>, bool)> = Mutex::new((None, false));

pub fn start() {
    *SEEN.lock() = (None, false);
}

/// What the mod came as. The first one noted stands (a BNP inside a zip is a BNP).
pub fn came_as(kind: &'static str) {
    SEEN.lock().0.get_or_insert(kind);
}

pub fn is_wii_u() {
    SEEN.lock().1 = true;
}

fn key(name: &str, version: &str) -> String {
    format!("{name}|{version}")
}

fn read(dir: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(dir.join(ORIGINS)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Notes what the mod just converted came as. `dir` is the folder the profiles are in.
pub fn finish(dir: &Path, name: &str, version: &str) {
    let (kind, wii_u) = *SEEN.lock();
    let Some(kind) = kind else { return };
    let mut origins = read(dir);
    origins.insert(key(name, version), format!("{kind}, {}", if wii_u { "Wii U" } else { "Switch" }));
    if let Ok(text) = serde_json::to_string_pretty(&origins) {
        if let Err(e) = fs::write(dir.join(ORIGINS), text) {
            log::warn!("Could not note where {name} came from: {e}");
        }
    }
}

/// A mod's text with what would break the list taken out: one line, no line that starts as a note.
fn clean(text: &str) -> String {
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    text.trim().trim_start_matches('#').trim().to_string()
}

/// The list of deployed mods. `mods` are the enabled ones in load order, the one that wins last.
pub fn list(dir: &Path, profile: &str, mods: &[Mod]) -> String {
    let origins = read(dir);
    let mut out = format!("# SWMM+ {}, profile {}\n", env!("CARGO_PKG_VERSION"), clean(profile));
    for mod_ in mods.iter().rev() {
        let meta = &mod_.meta;
        let origin = origins.get(&key(&meta.name, &meta.version)).map(String::as_str).unwrap_or("packaged, Switch");
        let mut line = format!("{} {} ({origin})", clean(&meta.name), clean(&meta.version));
        if !mod_.enabled_options.is_empty() {
            let options: Vec<String> = mod_.enabled_options.iter().map(|o| clean(&o.name)).collect();
            line.push_str(&format!(" [options: {}]", options.join(", ")));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Writes the list at the root of the merged game files, or takes it away when no mod is enabled.
pub fn write_list(content: &Path, dir: &Path, profile: &str, mods: &[Mod]) {
    let path = content.join(LIST_NAME);
    let result = if mods.is_empty() {
        if path.is_file() { fs::remove_file(&path) } else { Ok(()) }
    } else {
        fs::create_dir_all(content).and_then(|_| fs::write(&path, list(dir, profile, mods)))
    };
    if let Err(e) = result {
        log::warn!("Could not write the list of mods for Savage Wind's log: {e}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_mods_text_cant_break_the_list() {
        assert_eq!(clean("  # Linkle\r\nsecond line "), "Linkle  second line");
        assert_eq!(key("Linkle", "1.0.2"), "Linkle|1.0.2");
    }
}
