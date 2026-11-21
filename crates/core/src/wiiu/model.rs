//! Wii U texture files to the Switch's.
//!
//! Both consoles keep a model's textures in a model file ("FRES") of their own: icons (.sbitemico), a model's
//! textures (.Tex1.sbfres) and so on. The Wii U's holds each texture as a record with its pixels; the Switch's holds
//! one texture archive ("textures.bntx") as an attached file. A Wii U file with nothing but textures is rebuilt
//! here as the Switch's. Files with models or animations aren't: those are other formats again.

use anyhow_ext::{bail, Context, Result};

use uk_content::layout::bntx::Bntx;

use super::tex::{build_bntx, gx2_kind, gx2_untile, Texture};

fn be32(data: &[u8], at: usize) -> Result<u32> {
    let bytes = data.get(at..at + 4).context("the file ends early")?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// An offset stored relative to its own place; `None` when it is zero.
fn pointer(data: &[u8], at: usize) -> Result<Option<usize>> {
    let offset = be32(data, at)? as i32;
    if offset == 0 {
        return Ok(None);
    }
    let to = at as i64 + offset as i64;
    if to < 0 || to as usize > data.len() {
        bail!("an offset points outside the file");
    }
    Ok(Some(to as usize))
}

fn text(data: &[u8], at: usize) -> Result<String> {
    let rest = data.get(at..).context("a name is outside the file")?;
    let end = rest.iter().position(|&b| b == 0).context("a name has no end")?;
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// The kinds of things a Wii U model file lists, in the order of its header. Textures are the second.
const GROUPS: usize = 12;
const TEXTURES: usize = 1;

/// Whether this is a Wii U model file.
pub fn is_wii_u(data: &[u8]) -> bool {
    data.len() >= 10 && &data[..4] == b"FRES" && data[8..10] == [0xFE, 0xFF]
}

/// Whether a Wii U model file holds only textures.
pub fn only_textures(data: &[u8]) -> bool {
    is_wii_u(data)
        && data.len() >= 0x6C
        && (0..GROUPS).all(|group| {
            let count = u16::from_be_bytes([data[0x50 + 2 * group], data[0x51 + 2 * group]]);
            (group == TEXTURES) == (count > 0)
        })
}

/// One level of a Wii U texture in plain rows. Smaller levels are stored padded to powers of two.
#[allow(clippy::too_many_arguments)]
fn level(
    data: &[u8],
    width: u32,
    height: u32,
    block: u32,
    bytes: u32,
    tile_mode: u8,
    swizzle: u8,
    padded: bool,
) -> Result<Vec<u8>> {
    let (w, h) = (width.div_ceil(block), height.div_ceil(block));
    if !padded {
        return gx2_untile(data, w, h, bytes, tile_mode, swizzle, 0);
    }
    let (pw, ph) = (width.next_power_of_two().div_ceil(block), height.next_power_of_two().div_ceil(block));
    let whole = gx2_untile(data, pw, ph, bytes, tile_mode, swizzle, 0)?;
    let row = (w * bytes) as usize;
    let mut out = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        let start = y * (pw * bytes) as usize;
        out.extend_from_slice(&whole[start..start + row]);
    }
    Ok(out)
}

/// Where a texture record's pixels are in the file.
fn pixel_range(data: &[u8], at: usize) -> Result<Option<std::ops::Range<usize>>> {
    let start = pointer(data, at + 0xB0)?;
    Ok(start.map(|start| start..start + be32(data, at + 0x24).map(|s| s as usize).unwrap_or(0)))
}

fn texture(data: &[u8], at: usize, pixel_ranges: &[std::ops::Range<usize>]) -> Result<Texture> {
    if data.get(at..at + 4) != Some(b"FTEX") {
        bail!("a texture record isn't where the file says");
    }
    let name = text(data, pointer(data, at + 0xA8)?.context("a texture has no name")?)?;
    let field = |offset: usize| be32(data, at + offset);
    let (dimension, width, height, depth, levels) =
        (field(0x04)?, field(0x08)?, field(0x0C)?, field(0x10)?, field(0x14)?);
    let (format, samples, tile_mode) = (field(0x18)?, field(0x1C)?, field(0x34)?);
    if dimension != 1 || depth != 1 || samples != 0 {
        bail!("{name}: only plain two-dimensional textures are supported (this one is of kind {dimension})");
    }
    if width == 0 || height == 0 || width > 16384 || height > 16384 || levels == 0 || levels > 14 {
        bail!("{name}: {width}x{height} with {levels} levels isn't a size a texture can have");
    }
    let channels = data.get(at + 0x88..at + 0x8C).context("the file ends early")?;
    let kind = gx2_kind(format, [channels[0], channels[1], channels[2], channels[3]])
        .with_context(|| format!("{name}: texture format {format:#x} isn't supported"))?;
    let tile_mode = u8::try_from(tile_mode).ok().filter(|m| [0, 1, 2, 4].contains(m));
    let tile_mode = tile_mode.with_context(|| format!("{name}: its storage order isn't supported"))?;
    let swizzle = field(0x38)?;
    let swizzle = ((swizzle >> 8) & 7) as u8;

    let pixels_at = pointer(data, at + 0xB0)?.with_context(|| format!("{name}: it has no pixels in this file"))?;
    let size = field(0x24)? as usize;
    let pixels = data.get(pixels_at..pixels_at + size).with_context(|| format!("{name}: its pixels end early"))?;
    let first = level(pixels, width, height, kind.block, kind.bytes, tile_mode, swizzle, false)
        .with_context(|| name.clone())?;

    let mut mips = Vec::new();
    // The smaller levels are a block of their own. Files that don't say where it is would have it where the first
    // level's offset says, counted from the pixels. A model's first texture file (.Tex1) says its textures have
    // smaller levels and doesn't hold them: they are in the second file, and where this one says they are is past
    // its end or in the next texture's pixels.
    let start = match pointer(data, at + 0xB4)? {
        Some(start) => start,
        None => pixels_at + field(0x44)? as usize,
    };
    let block = start..start + field(0x2C)? as usize;
    let held = !block.is_empty()
        && block.end <= data.len()
        && !pixel_ranges.iter().any(|r| r.start < block.end && block.start < r.end);
    if levels > 1 && held {
        let all = &data[block];
        for i in 1..levels {
            // The second level starts the block; the rest say where they start in it.
            let offset = if i == 1 { 0 } else { field(0x44 + 4 * (i as usize - 1))? as usize };
            let from = all.get(offset..).with_context(|| format!("{name}: level {i} is outside the file"))?;
            let (w, h) = ((width >> i).max(1), (height >> i).max(1));
            // A level too small for the large tiles is stored in small ones.
            let (tiles_w, tiles_h) =
                (w.next_power_of_two().div_ceil(kind.block), h.next_power_of_two().div_ceil(kind.block));
            let small = tiles_w < 32 * (4 / kind.bytes).max(1) || tiles_h < 16;
            let mode = if tile_mode == 4 && small { 2 } else { tile_mode };
            mips.push(
                level(from, w, h, kind.block, kind.bytes, mode, swizzle, true)
                    .with_context(|| format!("{name}: level {i}"))?,
            );
        }
    }
    Ok(Texture { name, width, height, kind, layers: vec![first], mips, array: false })
}

/// The textures of a Wii U model file, and the name the file gives itself.
pub fn read_textures(data: &[u8]) -> Result<(String, Vec<Texture>)> {
    if !is_wii_u(data) {
        bail!("not a Wii U model file");
    }
    let name = text(data, pointer(data, 0x14)?.context("the file has no name")?)?;
    let Some(list) = pointer(data, 0x20 + 4 * TEXTURES)? else {
        return Ok((name, Vec::new()));
    };
    let count = be32(data, list + 4)? as usize;
    if count > 4096 {
        bail!("the file says it holds {count} textures");
    }
    let mut records = Vec::with_capacity(count);
    for i in 1..=count {
        records.push(pointer(data, list + 8 + 16 * i + 12)?.context("a texture is missing")?);
    }
    let mut pixel_ranges = Vec::with_capacity(count);
    for &record in &records {
        pixel_ranges.extend(pixel_range(data, record)?);
    }
    let mut textures = Vec::with_capacity(count);
    for record in records {
        textures.push(texture(data, record, &pixel_ranges)?);
    }
    Ok((name, textures))
}

/// Where the Switch's model file keeps things when all it holds is a texture archive.
const HEADER_END: usize = 0xF0;
const STRINGS: usize = 0x128;
const ARCHIVE: usize = 0x1000;
const ARCHIVE_NAME: &str = "textures.bntx";

/// The Switch's model file that holds nothing but a texture archive: what the game's own icon and texture files
/// are, byte for byte.
pub fn build_fres(name: &str, bntx: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; STRINGS + 0x14];
    let put16 = |out: &mut Vec<u8>, at: usize, v: u16| out[at..at + 2].copy_from_slice(&v.to_le_bytes());
    let put32 = |out: &mut Vec<u8>, at: usize, v: usize| out[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
    let put64 = |out: &mut Vec<u8>, at: usize, v: usize| out[at..at + 8].copy_from_slice(&(v as u64).to_le_bytes());
    let string = |out: &mut Vec<u8>, text: &str| -> usize {
        let at = out.len();
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out.push(0);
        out.resize(out.len().div_ceil(2) * 2, 0);
        at
    };
    // The strings: an empty one, then by length and name as the game's files have them.
    let empty = out.len();
    out.extend_from_slice(&[0; 4]);
    let (archive_name, file_name) = (string(&mut out, ARCHIVE_NAME), string(&mut out, name));
    let strings_end = out.len();
    out.resize(ARCHIVE, 0);
    out.extend_from_slice(bntx);
    out.resize(out.len().div_ceil(0x1000) * 0x1000, 0);
    let relocations = out.len();

    out[..8].copy_from_slice(b"FRES    ");
    out[8..0x10].copy_from_slice(&[3, 0, 5, 0, 0xFF, 0xFE, 0x0C, 0]);
    put32(&mut out, 0x10, file_name + 2);
    put16(&mut out, 0x16, STRINGS as u16);
    put32(&mut out, 0x18, relocations);
    put64(&mut out, 0x20, file_name);
    put64(&mut out, 0x98, HEADER_END);
    put64(&mut out, 0xA0, HEADER_END + 0x10);
    put64(&mut out, 0xB0, empty);
    put32(&mut out, 0xB8, strings_end - STRINGS);
    put16(&mut out, 0xC8, 1);
    put32(&mut out, 0xD0, 0x22);
    // The attached file.
    put64(&mut out, HEADER_END, ARCHIVE);
    put32(&mut out, HEADER_END + 8, bntx.len());
    // Its name's dictionary: the root and the one entry.
    let dictionary = HEADER_END + 0x10;
    put32(&mut out, dictionary + 4, 1);
    put32(&mut out, dictionary + 8, 0xFFFF_FFFF);
    put16(&mut out, dictionary + 0xC, 1);
    put64(&mut out, dictionary + 0x10, empty);
    put32(&mut out, dictionary + 0x18, 3);
    put16(&mut out, dictionary + 0x1E, 1);
    put64(&mut out, dictionary + 0x20, archive_name);
    out[STRINGS..STRINGS + 4].copy_from_slice(b"_STR");
    put32(&mut out, STRINGS + 8, relocations - STRINGS);
    put32(&mut out, STRINGS + 0x10, 2);

    // Where the loader finds the offsets above: five sections, of which the first and last hold something.
    out.resize(relocations + 0xB0, 0);
    out[relocations..relocations + 4].copy_from_slice(b"_RLT");
    put32(&mut out, relocations + 4, relocations);
    put32(&mut out, relocations + 8, 5);
    for section in 0..5 {
        let at = relocations + 0x10 + 0x18 * section;
        let (position, size, first, count) = match section {
            0 => (0, strings_end, 0, 4),
            4 => (ARCHIVE, relocations - ARCHIVE, 4, 1),
            _ => (strings_end, 0, 4, 0),
        };
        put32(&mut out, at + 8, position);
        put32(&mut out, at + 0xC, size);
        put32(&mut out, at + 0x10, first);
        put32(&mut out, at + 0x14, count);
    }
    let entries: [(usize, u16, u8, u8); 5] = [
        (0x20, 7, 1, 1),
        (0x98, 1, 2, 0),
        (0xB0, 2, 1, 0x0B),
        (dictionary + 0x20, 1, 1, 0),
        (HEADER_END, 1, 1, 0),
    ];
    for (i, (position, structures, offsets, padding)) in entries.into_iter().enumerate() {
        let at = relocations + 0x88 + 8 * i;
        put32(&mut out, at, position);
        put16(&mut out, at + 4, structures);
        out[at + 6] = offsets;
        out[at + 7] = padding;
    }
    let size = out.len();
    put32(&mut out, 0x1C, size);
    out
}

/// The texture archive of a Switch model file that holds only that.
pub fn switch_archive(data: &[u8]) -> Option<&[u8]> {
    if data.get(..8)? != b"FRES    " || data.get(0xC8..0xCA)? != [1, 0] {
        return None;
    }
    let le = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize);
    let entry = le(0x98)?;
    let (start, size) = (le(entry)?, le(entry + 8)?);
    data.get(start..start + size).filter(|d| d.starts_with(b"BNTX"))
}

/// The game's texture of this name in its texture archive: whether its first level is the same, and how many
/// levels it has. `None` when the game has no such texture.
fn in_game(archive: &[u8], texture: &Texture) -> Option<(bool, usize)> {
    let le = |at: usize| Some(u32::from_le_bytes(archive.get(at..at + 4)?.try_into().ok()?) as usize);
    let find = || -> Option<Option<(bool, usize)>> {
        let (count, table) = (le(0x24)?, le(0x28)?);
        for i in 0..count {
            let record = le(table + 8 * i)?;
            let name = le(record + 0x60)?;
            let length = u16::from_le_bytes(archive.get(name..name + 2)?.try_into().ok()?) as usize;
            if archive.get(name + 2..name + 2 + length)? != texture.name.as_bytes() {
                continue;
            }
            let info = record + 0x10;
            let kind = texture.kind;
            let levels = u16::from_le_bytes(archive.get(info + 6..info + 8)?.try_into().ok()?) as usize;
            if (le(info + 0x0C)?, le(info + 0x14)?, le(info + 0x18)?, le(info + 0x20)?)
                != (kind.nx_format as usize, texture.width as usize, texture.height as usize, 1)
                || le(info + 0x48)? != kind.channels as usize
            {
                return Some(Some((false, levels)));
            }
            let pixels = archive.get(le(le(record + 0x70)?)?..)?;
            let (w, h) = (texture.width.div_ceil(kind.block), texture.height.div_ceil(kind.block));
            let theirs = super::tex::tegra_unswizzle_at(pixels, w, h, kind.bytes, le(info + 0x24)? as u32)?;
            return Some(Some((texture.layers.first() == Some(&theirs), levels)));
        }
        Some(None)
    };
    find().flatten()
}

/// Gives a texture that came at full size only its smaller levels, down to one pixel.
fn add_levels(texture: &mut Texture) {
    if !texture.mips.is_empty() || texture.layers.len() != 1 {
        return;
    }
    let count = texture.width.max(texture.height).ilog2() as usize;
    match super::mips::smaller_levels(texture.kind, texture.width, texture.height, &texture.layers[0], count) {
        Some(levels) => texture.mips = levels,
        None => log::debug!("{} keeps one level: its format can't be packed here", texture.name),
    }
}

/// A Wii U model file that holds only textures, as the Switch's. Returns how many textures it changes.
///
/// `game` is the game's own Switch file of that name, when there is one. Textures the mod leaves as they are come
/// from it, with their smaller levels, which a Wii U model's texture file doesn't have; so do textures only the
/// Switch file has.
pub fn convert_texture_file(data: &[u8], game: Option<&[u8]>) -> Result<(Vec<u8>, usize)> {
    let (name, mut textures) = read_textures(data)?;
    if textures.is_empty() {
        bail!("it holds no textures");
    }
    // A model's texture file: its textures are seen from afar and need their smaller levels.
    let of_a_model = name.ends_with(".Tex1");
    let game = game.and_then(switch_archive).and_then(|archive| Some((archive, Bntx::parse(archive)?)));
    let bntx = match game {
        Some((archive, mut whole)) => {
            let mut changed = Vec::new();
            for mut texture in textures {
                match in_game(archive, &texture) {
                    Some((true, _)) => continue,
                    Some((false, levels)) if levels > 1 => add_levels(&mut texture),
                    None if of_a_model => add_levels(&mut texture),
                    _ => {}
                }
                changed.push(texture);
            }
            textures = changed;
            if !textures.is_empty() {
                let changed = build_bntx("textures", &textures)?;
                whole.merge(Bntx::parse(&changed).context("the texture archive didn't come out readable")?);
            }
            whole.to_binary()
        }
        None => {
            if of_a_model {
                textures.iter_mut().for_each(add_levels);
            }
            build_bntx("textures", &textures)?
        }
    };
    // A model's textures are "Name.Tex1" on the Wii U and "Name.Tex" on the Switch.
    let name = match name.strip_suffix(".Tex1") {
        Some(stem) => format!("{stem}.Tex"),
        None => name,
    };
    Ok((build_fres(&name, &bntx), textures.len()))
}

/// The name a converted texture file has on the Switch, when it differs: `Name.Tex1.sbfres` is `Name.Tex.sbfres`.
pub fn switch_name(name: &str) -> Option<String> {
    [".Tex1.sbfres", ".Tex1.bfres"].iter().find_map(|suffix| {
        let stem = name.strip_suffix(suffix)?;
        Some(format!("{stem}.Tex{}", &suffix[5..]))
    })
}

/// The Wii U's second texture file of a model: small copies of the first file's textures, shown while that one
/// loads. The Switch has no such file.
pub fn is_second_texture_file(name: &str) -> bool {
    name.ends_with(".Tex2.sbfres") || name.ends_with(".Tex2.bfres")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A Wii U model file with one 32-bit texture in plain rows and, with `levels` over one, smaller levels.
    pub(crate) fn wii_u_file(name: &str, texture_name: &str, width: u32, height: u32, levels: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        let put = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_be_bytes());
        d[..4].copy_from_slice(b"FRES");
        d[4..10].copy_from_slice(&[4, 5, 0, 3, 0xFE, 0xFF]);
        d[0x53] = 1; // one texture
        let (list, record, names) = (0x6C, 0x94, 0x160);
        put(&mut d, 0x14, (names - 0x14) as u32);
        put(&mut d, 0x24, (list - 0x24) as u32);
        put(&mut d, list + 4, 1);
        put(&mut d, list + 8 + 16 + 12, (record - (list + 8 + 16 + 12)) as u32);
        d[names..names + name.len()].copy_from_slice(name.as_bytes());
        let texture_at = names + 0x40;
        d[texture_at..texture_at + texture_name.len()].copy_from_slice(texture_name.as_bytes());
        d[record..record + 4].copy_from_slice(b"FTEX");
        let size = width * height * 4;
        for (at, v) in [(0x04, 1), (0x08, width), (0x0C, height), (0x10, 1), (0x14, levels), (0x18, 0x41A)] {
            put(&mut d, record + at, v);
        }
        put(&mut d, record + 0x24, size);
        put(&mut d, record + 0x38, 0x000D_0000);
        d[record + 0x88..record + 0x8C].copy_from_slice(&[0, 1, 2, 3]);
        put(&mut d, record + 0xA8, (texture_at - (record + 0xA8)) as u32);
        put(&mut d, record + 0xB0, (0x200 - (record + 0xB0)) as u32);
        d.extend((0..size).map(|i| i as u8));
        if levels > 1 {
            let start = d.len();
            put(&mut d, record + 0xB4, (start - (record + 0xB4)) as u32);
            let mut offset = 0u32;
            for i in 1..levels {
                if i > 1 {
                    put(&mut d, record + 0x44 + 4 * (i as usize - 1), offset);
                }
                let (w, h) = ((width >> i).max(1).next_power_of_two(), (height >> i).max(1).next_power_of_two());
                d.extend((0..w * h * 4).map(|p| (p + i) as u8));
                offset += w * h * 4;
            }
            put(&mut d, record + 0x2C, offset);
        }
        d
    }

    #[test]
    fn a_texture_file_becomes_the_switch_one() {
        let wii_u = wii_u_file("Thing.bitemico", "Thing", 12, 6, 3);
        assert!(only_textures(&wii_u));
        let (name, textures) = read_textures(&wii_u).unwrap();
        assert_eq!((name.as_str(), textures.len()), ("Thing.bitemico", 1));
        let texture = &textures[0];
        assert_eq!((texture.name.as_str(), texture.width, texture.height), ("Thing", 12, 6));
        assert_eq!(texture.kind.nx_format, 0x0B06);
        assert_eq!(texture.kind.channels, 0x0504_0302);
        assert_eq!(texture.layers[0], (0..12 * 6 * 4).map(|i| i as u8).collect::<Vec<_>>());
        // 6x3 stored 8 wide, 3x1 stored 4 wide.
        assert_eq!(texture.mips.len(), 2);
        assert_eq!(texture.mips[0].len(), 6 * 3 * 4);
        assert_eq!(texture.mips[0][24..28], [33, 34, 35, 36]);
        assert_eq!(texture.mips[1], (0..3 * 4).map(|p| (p + 2) as u8).collect::<Vec<_>>());

        let (switch, count) = convert_texture_file(&wii_u, None).unwrap();
        assert_eq!(count, 1);
        assert_eq!(&switch[..8], b"FRES    ");
        let archive = switch_archive(&switch).unwrap();
        let parsed = uk_content::layout::bntx::Bntx::parse(archive).unwrap();
        assert_eq!(parsed.file_name, "textures");
        assert_eq!(parsed.textures[0].name, "Thing");
        let size = u32::from_le_bytes(switch[0x1C..0x20].try_into().unwrap()) as usize;
        assert_eq!(size, switch.len());
    }

    #[test]
    fn a_models_textures_get_their_levels_and_the_games_own() {
        assert_eq!(switch_name("content/Model/A.Tex1.sbfres").as_deref(), Some("content/Model/A.Tex.sbfres"));
        assert_eq!(switch_name("Model/A.Tex1.bfres").as_deref(), Some("Model/A.Tex.bfres"));
        assert_eq!(switch_name("Model/A.sbfres"), None);
        assert!(is_second_texture_file("Model/A.Tex2.sbfres") && !is_second_texture_file("Model/A.Tex1.sbfres"));

        // The file says three levels and holds one, as a model's first texture file does.
        let mut wii_u = wii_u_file("Thing.Tex1", "Thing_Alb", 8, 8, 1);
        wii_u[0x94 + 0x17] = 3;
        let levels = |file: &[u8]| {
            let archive = switch_archive(file).unwrap();
            let record = u32::from_le_bytes(archive[0x28..0x2C].try_into().unwrap()) as usize;
            let record = u32::from_le_bytes(archive[record..record + 4].try_into().unwrap()) as usize;
            u16::from_le_bytes(archive[record + 0x16..record + 0x18].try_into().unwrap())
        };
        let (alone, changed) = convert_texture_file(&wii_u, None).unwrap();
        assert_eq!((changed, levels(&alone)), (1, 4));
        assert!(alone.windows(9).any(|w| w == b"Thing.Tex"));
        assert!(!alone.windows(10).any(|w| w == b"Thing.Tex1"));

        // Against a game that has the same texture, nothing changes and the game's is kept.
        let (again, changed) = convert_texture_file(&wii_u, Some(&alone)).unwrap();
        assert_eq!(changed, 0);
        assert_eq!(switch_archive(&again), switch_archive(&alone));
        // A changed pixel makes it the mod's, with new levels.
        let last = wii_u.len() - 1;
        wii_u[last] ^= 0xFF;
        let (modded, changed) = convert_texture_file(&wii_u, Some(&alone)).unwrap();
        assert_eq!((changed, levels(&modded)), (1, 4));
        assert_ne!(switch_archive(&modded), switch_archive(&alone));
    }

    #[test]
    fn files_with_more_than_textures_are_refused() {
        let mut wii_u = wii_u_file("Thing.bfres", "Thing", 8, 8, 1);
        wii_u[0x51] = 1; // a model
        assert!(is_wii_u(&wii_u) && !only_textures(&wii_u));
        let mut odd = wii_u_file("Thing.bfres", "Thing", 8, 8, 1);
        odd[0x94 + 0x1B] = 0x99;
        assert!(convert_texture_file(&odd, None).is_err());
    }
}
