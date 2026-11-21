//! Wii U layouts (.bflyt) and layout animations (.bflan) to the Switch's.
//!
//! The Wii U game's are version 5.2, big endian; the Switch game's are version 8.0, little endian. Most sections
//! only need their numbers turned around. Four changed shape between the versions: texture names lost their
//! ".bflim", materials have their flags first and a new field after them, text boxes have one more field, and an
//! animation's header and group names grew.
//!
//! Every size and offset is checked against the file. A layout with something this doesn't know (a parts pane, an
//! unknown section) is an error, never a guess: a wrong layout crashes the game.

use anyhow_ext::{bail, Context, Result};

fn be16(d: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(d.get(at..at + 2).context("file ends early")?.try_into()?))
}

fn be32(d: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(d.get(at..at + 4).context("file ends early")?.try_into()?))
}

fn off(d: &[u8], at: usize) -> Result<usize> {
    Ok(be32(d, at)? as usize)
}

/// Turns `count` values of `width` bytes around in place.
fn swap(d: &mut [u8], at: usize, width: usize, count: usize) -> Result<()> {
    let range = d.get_mut(at..at + width * count).context("file ends early")?;
    for value in range.chunks_mut(width) {
        value.reverse();
    }
    Ok(())
}

fn cstr(d: &[u8], at: usize) -> Result<&[u8]> {
    let rest = d.get(at..).context("file ends early")?;
    let len = rest.iter().position(|&b| b == 0).context("a name has no end")?;
    Ok(&rest[..len])
}

fn pad4(out: &mut Vec<u8>) {
    out.resize(out.len().div_ceil(4) * 4, 0);
}

/// The sections of a big-endian file, each with its 8-byte header.
fn sections<'a>(data: &'a [u8], magic: &[u8; 4]) -> Result<Vec<&'a [u8]>> {
    if data.len() < 0x14 || &data[..4] != magic || data[4..6] != [0xFE, 0xFF] {
        bail!("not a Wii U file of this kind");
    }
    let mut at = be16(data, 6)? as usize;
    let mut out = Vec::new();
    for _ in 0..be16(data, 0x10)? {
        let size = off(data, at + 4)?;
        if size < 8 {
            bail!("a section says it is {size} bytes");
        }
        out.push(data.get(at..at + size).context("a section runs past the end of the file")?);
        at += size;
    }
    Ok(out)
}

/// The Switch's file from converted sections.
fn assemble(magic: &[u8; 4], sections: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&[0xFF, 0xFE, 0x14, 0x00, 0x00, 0x00, 0x00, 0x08]);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(sections.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    for mut section in sections {
        pad4(&mut section);
        let size = section.len() as u32;
        section[4..8].copy_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&section);
    }
    let size = out.len() as u32;
    out[0x0C..0x10].copy_from_slice(&size.to_le_bytes());
    out
}

/// A name table (textures or fonts): a count, offsets from just after the count, the names.
fn names(s: &[u8]) -> Result<Vec<&[u8]>> {
    (0..be16(s, 8)? as usize).map(|i| cstr(s, 0x0C + off(s, 0x0C + 4 * i)?)).collect()
}

fn write_names(magic: &[u8], names: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(magic);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(names.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    let mut at = 4 * names.len();
    for name in names {
        out.extend_from_slice(&(at as u32).to_le_bytes());
        at += name.len() + 1;
    }
    for name in names {
        out.extend_from_slice(name);
        out.push(0);
    }
    out
}

fn without_bflim(name: &[u8]) -> &[u8] {
    name.strip_suffix(b".bflim").unwrap_or(name)
}

/// The part every pane starts with: flags, name, user data, then ten floats (position, rotation, scale, size).
const PANE_END: usize = 0x54;

fn pane(s: &mut [u8]) -> Result<()> {
    swap(s, 0x2C, 4, 10)
}

/// Vertex colours, a material and texture coordinates: a picture's content and a window's.
fn content(s: &mut [u8], at: usize) -> Result<()> {
    swap(s, at + 0x10, 2, 1)?;
    let coords = *s.get(at + 0x12).context("file ends early")? as usize;
    swap(s, at + 0x14, 4, 8 * coords)
}

fn material(m: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let flags = be32(m, 0x24)?;
    let count = |shift: u32, bits: u32| ((flags >> shift) & ((1 << bits) - 1)) as usize;
    out.extend_from_slice(&m[..0x1C]);
    out.extend_from_slice(&flags.to_le_bytes());
    // New in version 8; every material of the Switch game has this value.
    out.extend_from_slice(&0x0804_0200u32.to_le_bytes());
    out.extend_from_slice(&m[0x1C..0x24]);
    let mut at = 0x28;
    let copy = |out: &mut Vec<u8>, at: &mut usize, len: usize, swaps: &[(usize, usize, usize)]| -> Result<()> {
        let mut part = m.get(*at..*at + len).context("a material ends early")?.to_vec();
        for &(start, width, count) in swaps {
            swap(&mut part, start, width, count)?;
        }
        out.extend_from_slice(&part);
        *at += len;
        Ok(())
    };
    for _ in 0..count(0, 2) {
        copy(out, &mut at, 4, &[(0, 2, 1)])?; // texture: index, wrap modes
    }
    for _ in 0..count(2, 2) {
        copy(out, &mut at, 20, &[(0, 4, 5)])?; // texture matrix
    }
    for _ in 0..count(4, 2) {
        // Texture coordinate source: two bytes of settings, then room for a pointer the game fills in. The room is
        // 4 bytes on the Wii U and 12 on the Switch.
        copy(out, &mut at, 4, &[])?;
        at += 4;
        out.extend_from_slice(&[0; 12]);
    }
    for _ in 0..count(6, 3) {
        copy(out, &mut at, 4, &[])?; // combiner stage
    }
    if count(9, 1) == 1 {
        copy(out, &mut at, 8, &[(4, 4, 1)])?; // alpha test
    }
    if count(10, 1) == 1 {
        copy(out, &mut at, 4, &[])?; // blend
    }
    if count(12, 1) == 1 {
        copy(out, &mut at, 4, &[])?; // alpha blend
    }
    if count(14, 1) == 1 {
        copy(out, &mut at, 12, &[(0, 4, 3)])?; // indirect texture
    }
    if count(15, 2) != 0 {
        bail!("a material uses projected textures, which aren't supported yet");
    }
    if count(17, 1) == 1 {
        copy(out, &mut at, 8, &[])?; // font shadow colours
    }
    if flags >> 19 != 0 {
        bail!("a material has settings this doesn't know (flags {flags:#x})");
    }
    if at != m.len() {
        bail!("a material is {} bytes and its settings account for {at}", m.len());
    }
    Ok(())
}

fn materials(s: &[u8]) -> Result<Vec<u8>> {
    let count = be16(s, 8)? as usize;
    let starts = (0..count).map(|i| off(s, 0x0C + 4 * i)).collect::<Result<Vec<_>>>()?;
    let mut out = Vec::new();
    out.extend_from_slice(b"mat1");
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(count as u16).to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out.resize(0x0C + 4 * count, 0);
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(s.len());
        let at = out.len() as u32;
        out[0x0C + 4 * i..0x10 + 4 * i].copy_from_slice(&at.to_le_bytes());
        material(s.get(start..end).context("a material is out of place")?, &mut out)
            .with_context(|| format!("material {}", String::from_utf8_lossy(cstr(s, start).unwrap_or_default())))?;
    }
    Ok(out)
}

fn text_box(s: &[u8]) -> Result<Vec<u8>> {
    if s.len() < 0xA4 {
        bail!("a text box is too short");
    }
    let (text, name, per_char) = (off(s, 0x64)?, off(s, 0x80)?, off(s, 0xA0)?);
    if per_char != 0 {
        bail!("a text box moves its letters one by one, which isn't supported yet");
    }
    let mut out = s[..0xA0].to_vec();
    pane(&mut out)?;
    swap(&mut out, PANE_END, 2, 4)?; // text sizes in bytes, material, font
    swap(&mut out, 0x60, 4, 2)?; // italic tilt, where the text is
    swap(&mut out, 0x70, 4, 9)?; // font size, spacing, where the text's name is, shadow offset and scale
    swap(&mut out, 0x9C, 4, 1)?; // shadow tilt
    // One more field before the per-letter one in version 8, so everything after the header sits 4 bytes further.
    for at in [0x64, 0x80] {
        let value = u32::from_le_bytes(out[at..at + 4].try_into()?);
        if value != 0 {
            out[at..at + 4].copy_from_slice(&(value + 4).to_le_bytes());
        }
    }
    out.extend_from_slice(&[0; 8]);
    let mut rest = s[0xA4..].to_vec();
    if text != 0 {
        // The text is UTF-16 in the file's byte order, up to its name or the end of the section.
        let start = text.checked_sub(0xA4).context("a text box's text is out of place")?;
        let end = if name > text { name - 0xA4 } else { rest.len() };
        let len = end.checked_sub(start).context("a text box's text is out of place")? / 2;
        swap(&mut rest, start, 2, len)?;
    }
    out.extend_from_slice(&rest);
    Ok(out)
}

fn window(s: &mut [u8]) -> Result<()> {
    pane(s)?;
    swap(s, PANE_END, 2, 8)?; // how far it stretches, frame sizes
    let frames = *s.get(0x64).context("file ends early")? as usize;
    let (content_at, table) = (off(s, 0x68)?, off(s, 0x6C)?);
    swap(s, 0x68, 4, 2)?;
    content(s, content_at)?;
    for i in 0..frames {
        let frame = off(s, table + 4 * i)?;
        swap(s, table + 4 * i, 4, 1)?;
        swap(s, frame, 2, 1)?;
    }
    Ok(())
}

fn user_data(s: &mut [u8]) -> Result<()> {
    let count = be16(s, 8)? as usize;
    swap(s, 8, 2, 1)?;
    for i in 0..count {
        let entry = 0x0C + 12 * i;
        let (data, len, kind) = (off(s, entry + 4)?, be16(s, entry + 8)? as usize, s[entry + 10]);
        swap(s, entry, 4, 2)?;
        swap(s, entry + 8, 2, 1)?;
        match kind {
            0 => (),
            1 | 2 => swap(s, entry + data, 4, len)?,
            other => bail!("user data of kind {other} isn't supported yet"),
        }
    }
    Ok(())
}

fn control(s: &mut [u8]) -> Result<()> {
    let (pane_names, panes, anims) = (off(s, 0x0C)?, be16(s, 0x10)? as usize, be16(s, 0x12)? as usize);
    let (pane_params, anim_params) = (off(s, 0x14)?, off(s, 0x18)?);
    swap(s, 0x08, 4, 2)?;
    swap(s, 0x10, 2, 2)?;
    swap(s, 0x14, 4, 2)?;
    // Three tables of offsets: the animations' names after the panes' names, and the two parameter name tables.
    let tables = [(pane_names + 24 * panes, anims), (pane_params, panes), (anim_params, anims)];
    let mut done: Vec<usize> = Vec::new();
    for (at, count) in tables {
        if count != 0 && !done.contains(&at) {
            swap(s, at, 4, count)?;
            done.push(at);
        }
    }
    Ok(())
}

/// A pane that shows another layout, with what it overrides in it: a table of properties, the other layout's name,
/// then the overriding panes and user data the table points at. Overrides that keep their size are converted where
/// they are; a text box grows by 4 bytes in version 8 and isn't handled yet.
fn parts(s: &mut [u8]) -> Result<()> {
    pane(s)?;
    let count = off(s, PANE_END)?;
    swap(s, PANE_END, 4, 3)?; // how many properties, how much it is magnified
    for i in 0..count {
        let entry = PANE_END + 0x0C + 0x28 * i;
        let (property, data, info) = (off(s, entry + 0x1C)?, off(s, entry + 0x20)?, off(s, entry + 0x24)?);
        swap(s, entry + 0x1C, 4, 3)?;
        if info != 0 {
            bail!("a parts pane overrides a pane's placement, which isn't supported yet");
        }
        for at in [property, data] {
            if at == 0 {
                continue;
            }
            let size = off(s, at + 4)?;
            let inner = s.get_mut(at..at + size).context("a parts pane's override is out of place")?;
            let magic: [u8; 4] = inner[..4].try_into()?;
            match &magic {
                b"pan1" | b"bnd1" => pane(inner)?,
                b"pic1" => {
                    pane(inner)?;
                    content(inner, PANE_END)?;
                }
                b"wnd1" => window(inner)?,
                b"usd1" => user_data(inner)?,
                b"prt1" => parts(inner)?,
                other => bail!(
                    "a parts pane overrides a {} pane, which isn't supported yet",
                    String::from_utf8_lossy(other)
                ),
            }
            swap(inner, 4, 4, 1)?;
        }
    }
    Ok(())
}

/// A Wii U layout as the Switch game reads it.
pub fn convert_bflyt(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for section in sections(data, b"FLYT")? {
        let mut s = section.to_vec();
        let magic: [u8; 4] = s[..4].try_into()?;
        let converted = match &magic {
            b"lyt1" => {
                swap(&mut s, 0x0C, 4, 4)?;
                s
            }
            b"txl1" => write_names(b"txl1", &names(&s)?.into_iter().map(without_bflim).collect::<Vec<_>>()),
            b"fnl1" => write_names(b"fnl1", &names(&s)?),
            b"mat1" => materials(&s)?,
            b"pan1" | b"bnd1" => {
                pane(&mut s)?;
                s
            }
            b"pic1" => {
                pane(&mut s)?;
                content(&mut s, PANE_END)?;
                s
            }
            b"txt1" => text_box(&s)?,
            b"wnd1" => {
                window(&mut s)?;
                s
            }
            b"usd1" => {
                user_data(&mut s)?;
                s
            }
            b"grp1" => {
                swap(&mut s, 0x2A, 2, 1)?;
                s
            }
            b"cnt1" => {
                control(&mut s)?;
                s
            }
            b"pas1" | b"pae1" | b"grs1" | b"gre1" => s,
            b"prt1" => {
                parts(&mut s)?;
                s
            }
            other => bail!("it has a section this doesn't know ({})", String::from_utf8_lossy(other)),
        };
        out.push(converted);
    }
    Ok(assemble(b"FLYT", out))
}

fn animation_tag(s: &[u8]) -> Result<Vec<u8>> {
    let (groups, name, group_names) = (be16(s, 0x0A)? as usize, off(s, 0x0C)?, off(s, 0x10)?);
    if name != 0x1C || group_names < name || s.len() != group_names + 28 * groups {
        bail!("an animation's header isn't laid out as expected");
    }
    let mut out = s[..0x14].to_vec();
    swap(&mut out, 0x08, 2, 2)?;
    out[0x0C..0x10].copy_from_slice(&(name as u32 + 4).to_le_bytes());
    out[0x10..0x14].copy_from_slice(&(group_names as u32 + 4).to_le_bytes());
    out.extend_from_slice(&[0; 4]); // new in version 8
    let mut frames = s[0x14..0x1C].to_vec();
    swap(&mut frames, 0, 2, 2)?; // first and last frame
    out.extend_from_slice(&frames);
    out.extend_from_slice(&s[name..group_names]);
    for group in s[group_names..].chunks(28) {
        // Group names are 36 bytes in version 8.
        out.extend_from_slice(group);
        out.extend_from_slice(&[0; 8]);
    }
    Ok(out)
}

fn animation_keys(s: &mut [u8], target: usize) -> Result<()> {
    let (curve, keys, at) = (s[target + 2], be16(s, target + 4)? as usize, target + off(s, target + 8)?);
    swap(s, target + 4, 2, 1)?;
    swap(s, target + 8, 4, 1)?;
    match curve {
        // Step keys: frame, a 16-bit value, padding.
        1 => {
            for key in 0..keys {
                swap(s, at + 8 * key, 4, 1)?;
                swap(s, at + 8 * key + 4, 2, 1)?;
            }
        }
        // Curve keys: frame, value, slope.
        2 => swap(s, at, 4, 3 * keys)?,
        other => bail!("an animation has keys of kind {other}, which isn't supported yet"),
    }
    Ok(())
}

fn animation(s: &[u8]) -> Result<Vec<u8>> {
    let (textures, entries, table) = (be16(s, 0x0C)? as usize, be16(s, 0x0E)? as usize, off(s, 0x10)?);
    let names = (0..textures)
        .map(|i| Ok(without_bflim(cstr(s, 0x14 + off(s, 0x14 + 4 * i)?)?)))
        .collect::<Result<Vec<_>>>()?;
    let mut out = s[..0x14].to_vec();
    swap(&mut out, 0x08, 2, 1)?; // length in frames
    swap(&mut out, 0x0C, 2, 2)?;
    let mut at = 4 * names.len();
    for name in &names {
        out.extend_from_slice(&(at as u32).to_le_bytes());
        at += name.len() + 1;
    }
    for name in &names {
        out.extend_from_slice(name);
        out.push(0);
    }
    pad4(&mut out);
    // Everything from the entry table on keeps its shape and moves as one piece, by `moved` bytes.
    let new_table = out.len();
    let moved = new_table as i64 - table as i64;
    out[0x10..0x14].copy_from_slice(&(new_table as u32).to_le_bytes());
    let mut rest = s.to_vec();
    for i in 0..entries {
        let entry = off(s, table + 4 * i)?;
        let new_entry = (entry as i64 + moved) as u32;
        rest[table + 4 * i..table + 4 * i + 4].copy_from_slice(&new_entry.to_le_bytes());
        let (tags, kind) = (*s.get(entry + 28).context("file ends early")? as usize, s[entry + 29]);
        if kind > 1 {
            bail!("an animation drives user data, which isn't supported yet");
        }
        for t in 0..tags {
            let tag = entry + off(s, entry + 32 + 4 * t)?;
            swap(&mut rest, entry + 32 + 4 * t, 4, 1)?;
            let targets = *s.get(tag + 4).context("file ends early")? as usize;
            for g in 0..targets {
                let target = tag + off(s, tag + 8 + 4 * g)?;
                swap(&mut rest, tag + 8 + 4 * g, 4, 1)?;
                animation_keys(&mut rest, target)?;
            }
        }
    }
    out.extend_from_slice(rest.get(table..).context("an animation's table is out of place")?);
    Ok(out)
}

/// A Wii U layout animation as the Switch game reads it.
pub fn convert_bflan(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for section in sections(data, b"FLAN")? {
        out.push(match &section[..4] {
            b"pat1" => animation_tag(section)?,
            b"pai1" => animation(section)?,
            other => bail!("it has a section this doesn't know ({})", String::from_utf8_lossy(other)),
        });
    }
    Ok(assemble(b"FLAN", out))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn section(magic: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut s = magic.to_vec();
        s.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
        s.extend_from_slice(body);
        s
    }

    fn file(magic: &[u8; 4], sections: &[Vec<u8>]) -> Vec<u8> {
        let mut out = magic.to_vec();
        out.extend_from_slice(&[0xFE, 0xFF, 0x00, 0x14, 0x05, 0x02, 0x00, 0x00]);
        let size: usize = 0x14 + sections.iter().map(Vec::len).sum::<usize>();
        out.extend_from_slice(&(size as u32).to_be_bytes());
        out.extend_from_slice(&(sections.len() as u16).to_be_bytes());
        out.extend_from_slice(&[0; 2]);
        for s in sections {
            out.extend_from_slice(s);
        }
        out
    }

    fn le32(d: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn layout_sections_change_shape() {
        // Two texture names, the first ".bflim".
        let mut txl = vec![0, 2, 0, 0, 0, 0, 0, 8, 0, 0, 0, 0x12];
        txl.extend_from_slice(b"A^s.bflim\0");
        txl.extend_from_slice(b"B\0");
        // One material: a texture, a matrix, a coordinate source and a blend mode (flags 0x415).
        let mut mat = vec![0, 1, 0, 0, 0, 0, 0, 0x10];
        let mut name = b"M_Icon".to_vec();
        name.resize(28, 0);
        mat.extend_from_slice(&name);
        mat.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        mat.extend_from_slice(&0x415u32.to_be_bytes());
        mat.extend_from_slice(&[0, 3, 1, 1]);
        for value in [0.0f32, 0.0, 0.0, 1.0, 2.0] {
            mat.extend_from_slice(&value.to_be_bytes());
        }
        mat.extend_from_slice(&[9, 8, 0, 0, 0, 0, 0, 0]);
        mat.extend_from_slice(&[1, 4, 1, 0]);
        let out = convert_bflyt(&file(b"FLYT", &[section(b"txl1", &txl), section(b"mat1", &mat)])).unwrap();
        assert_eq!(&out[..0x0C], b"FLYT\xFF\xFE\x14\x00\x00\x00\x00\x08");
        assert_eq!(le32(&out, 0x0C) as usize, out.len());
        let txl = &out[0x14..];
        assert_eq!(&txl[..4], b"txl1");
        assert_eq!(le32(txl, 4), 28);
        assert_eq!(&txl[0x14..0x1A], b"A^s\0B\0");
        let mat = &txl[28..];
        assert_eq!(le32(mat, 4), 0x10 + 88);
        assert_eq!(le32(mat, 0x0C), 0x10);
        let m = &mat[0x10..];
        assert_eq!(le32(m, 0x1C), 0x415);
        assert_eq!(&m[0x20..0x24], &[0, 2, 4, 8]);
        assert_eq!(&m[0x24..0x2C], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&m[0x2C..0x30], &[3, 0, 1, 1]);
        assert_eq!(f32::from_le_bytes(m[0x40..0x44].try_into().unwrap()), 2.0);
        assert_eq!(&m[0x44..0x54], &[9, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&m[0x54..0x58], &[1, 4, 1, 0]);
    }

    #[test]
    fn a_material_that_does_not_add_up_is_refused() {
        let mut mat = vec![0, 1, 0, 0, 0, 0, 0, 0x10];
        mat.extend_from_slice(&[0; 36]);
        mat.extend_from_slice(&0x1u32.to_be_bytes());
        // Says one texture and holds none.
        assert!(convert_bflyt(&file(b"FLYT", &[section(b"mat1", &mat)])).is_err());
        assert!(convert_bflyt(&file(b"FLYT", &[section(b"xyz1", &[0; 0x60])])).is_err());
        assert!(convert_bflyt(b"FLYT\xFF\xFE").is_err());
    }

    #[test]
    fn text_moves_four_bytes_and_turns_around() {
        let mut body = vec![0u8; 0x9C];
        body[0x5C..0x60].copy_from_slice(&0xA4u32.to_be_bytes()); // text offset (section offset 0x64)
        body[0x78..0x7C].copy_from_slice(&0xA8u32.to_be_bytes()); // name offset (section offset 0x80)
        body.extend_from_slice(&[0x00, 0x41, 0x00, 0x00]); // "A"
        body.extend_from_slice(b"T_A\0");
        let out = convert_bflyt(&file(b"FLYT", &[section(b"txt1", &body)])).unwrap();
        let s = &out[0x14..];
        assert_eq!(le32(s, 4) as usize, 0xA4 + 8 + 4);
        assert_eq!(le32(s, 0x64), 0xA8);
        assert_eq!(le32(s, 0x80), 0xAC);
        assert_eq!(&s[0xA8..0xB0], b"\x41\x00\x00\x00T_A\0");
    }

    #[test]
    fn parts_panes() {
        // One property that overrides nothing, then the other layout's name.
        let mut body = vec![0u8; 0x4C];
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&1.0f32.to_be_bytes());
        body.extend_from_slice(&2.0f32.to_be_bytes());
        body.extend_from_slice(&[0; 0x28]);
        body.extend_from_slice(b"PaOther\0");
        let out = convert_bflyt(&file(b"FLYT", &[section(b"prt1", &body)])).unwrap();
        let s = &out[0x14..];
        assert_eq!(le32(s, 0x54), 1);
        assert_eq!(f32::from_le_bytes(s[0x5C..0x60].try_into().unwrap()), 2.0);
        assert_eq!(&s[0x88..0x90], b"PaOther\0");
        // A text override would have to grow: refused.
        let at = body.len() as u32 + 8;
        body[0x4C + 0x0C + 0x1C..0x4C + 0x0C + 0x20].copy_from_slice(&at.to_be_bytes());
        body.extend_from_slice(b"txt1\0\0\0\xA8");
        body.extend_from_slice(&[0; 0xA0]);
        assert!(convert_bflyt(&file(b"FLYT", &[section(b"prt1", &body)])).is_err());
    }

    #[test]
    fn animation_names_and_keys() {
        let mut tag = vec![0, 0, 0, 1, 0, 0, 0, 0x1C, 0, 0, 0, 0x20, 0, 0, 0, 0x32, 0, 0, 0, 0];
        tag.extend_from_slice(b"In\0\0");
        let mut group = b"G_In".to_vec();
        group.resize(28, 0);
        tag.extend_from_slice(&group);
        // One texture name, one entry with one tag, one target with one step key.
        let mut info = vec![0, 0x32, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0x24];
        info.extend_from_slice(&4u32.to_be_bytes());
        info.extend_from_slice(b"T^f.bflim\0\0\0");
        assert_eq!(info.len() + 8, 0x24);
        info.extend_from_slice(&0x28u32.to_be_bytes()); // entry table
        let mut entry = b"P_Pane".to_vec();
        entry.resize(28, 0);
        entry.extend_from_slice(&[1, 0, 0, 0]);
        entry.extend_from_slice(&0x24u32.to_be_bytes()); // tag, from the entry
        entry.extend_from_slice(b"FLTP");
        entry.extend_from_slice(&[1, 0, 0, 0]);
        entry.extend_from_slice(&0x0Cu32.to_be_bytes()); // target, from the tag
        entry.extend_from_slice(&[0, 0, 1, 0, 0, 1, 0, 0]);
        entry.extend_from_slice(&0x0Cu32.to_be_bytes()); // keys, from the target
        entry.extend_from_slice(&3.0f32.to_be_bytes());
        entry.extend_from_slice(&[0, 7, 0, 0]);
        info.extend_from_slice(&entry);
        let out = convert_bflan(&file(b"FLAN", &[section(b"pat1", &tag), section(b"pai1", &info)])).unwrap();
        let pat = &out[0x14..];
        assert_eq!(le32(pat, 4), 0x20 + 4 + 36);
        assert_eq!((le32(pat, 0x0C), le32(pat, 0x10)), (0x20, 0x24));
        assert_eq!(&pat[0x18..0x1C], &[0, 0, 0x32, 0]);
        let pai = &pat[0x48..];
        assert_eq!(&pai[..4], b"pai1");
        assert_eq!(&pai[0x18..0x1C], b"T^f\0");
        let table = le32(pai, 0x10) as usize;
        assert_eq!(table, 0x1C);
        let entry = le32(pai, table) as usize;
        assert_eq!(entry, 0x20);
        let tag = entry + le32(pai, entry + 32) as usize;
        assert_eq!(&pai[tag..tag + 4], b"FLTP");
        let target = tag + le32(pai, tag + 8) as usize;
        let keys = target + le32(pai, target + 8) as usize;
        assert_eq!(f32::from_le_bytes(pai[keys..keys + 4].try_into().unwrap()), 3.0);
        assert_eq!(&pai[keys + 4..keys + 6], &[7, 0]);
    }
}
