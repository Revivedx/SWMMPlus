//! Wii U fonts (.bffnt) to the Switch's.
//!
//! The Wii U game's are version 3, big endian, with the glyph sheets as raw Wii U textures. The Switch game's are
//! version 4.1, little endian, with the sheets as one array texture in a texture archive inside the file, and
//! character codes in 32 bits where version 3 has 16.

use anyhow_ext::{bail, Context, Result};

use super::tex::{build_bntx, flim_kind, gx2_untile, Kind, Texture, FONT_SHEET};

/// A font's sheet format. Text fonts are BC4 (one channel, the glyph's coverage); fonts of coloured icons use the
/// colour formats. The codes aren't a layout image's, so they are named by the layout image format that stores the
/// same thing.
fn sheet_kind(format: u16) -> Option<Kind> {
    match format {
        0x00 => flim_kind(0x09),
        0x08 => flim_kind(0x01),
        0x09 => flim_kind(0x0C),
        0x0A => flim_kind(0x0D),
        0x0B => flim_kind(0x0E),
        0x0C => Some(FONT_SHEET),
        0x0D => flim_kind(0x11),
        0x0E => flim_kind(0x14),
        0x0F => flim_kind(0x15),
        0x10 => flim_kind(0x16),
        0x11 => flim_kind(0x17),
        _ => None,
    }
}

fn be16(d: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(d.get(at..at + 2).context("file ends early")?.try_into()?))
}

fn be32(d: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(d.get(at..at + 4).context("file ends early")?.try_into()?))
}

fn le16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn le32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn set32(out: &mut [u8], at: usize, value: usize) {
    out[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

/// Where a section's data starts, checked against its name. The file points at data, 8 bytes into a section.
fn section(d: &[u8], data: usize, magic: &[u8; 4]) -> Result<usize> {
    if data < 8 || d.get(data - 8..data - 4) != Some(magic) {
        bail!("the font's {} section isn't where the file says", String::from_utf8_lossy(magic));
    }
    Ok(data)
}

/// A Wii U font as the Switch game reads it.
pub fn convert_bffnt(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 0x3C || &data[..4] != b"FFNT" || data[4..6] != [0xFE, 0xFF] {
        bail!("not a Wii U font");
    }
    let info = section(data, 0x1C, b"FINF")?;
    let sheets_at = section(data, be32(data, info + 0x0C)? as usize, b"TGLP")?;
    let mut widths_at = be32(data, info + 0x10)? as usize;
    let mut map_at = be32(data, info + 0x14)? as usize;

    let sheet_count = *data.get(sheets_at + 2).context("file ends early")? as usize;
    let sheet_size = be32(data, sheets_at + 4)? as usize;
    let format = be16(data, sheets_at + 0x0A)?;
    let (width, height) = (be16(data, sheets_at + 0x10)? as u32, be16(data, sheets_at + 0x12)? as u32);
    let pixels = be32(data, sheets_at + 0x14)? as usize;
    let format = format & 0x7FFF;
    let kind = sheet_kind(format)
        .with_context(|| format!("its glyph sheets are in format {format:#x}, which isn't supported yet"))?;
    let stored = data
        .get(pixels..pixels + sheet_size * sheet_count)
        .context("the font's glyph sheets run past the end of the file")?;
    let layers = (0..sheet_count)
        .map(|i| {
            gx2_untile(
                &stored[i * sheet_size..(i + 1) * sheet_size],
                width.div_ceil(kind.block),
                height.div_ceil(kind.block),
                kind.bytes,
                4,
                0,
                i as u32,
            )
        })
        .collect::<Result<Vec<_>>>()
        .context("the font's glyph sheets")?;
    let bntx = build_bntx("0", &[Texture {
        name: "0".into(),
        width,
        height,
        kind,
        layers,
        array: true,
    }])?;

    let mut out = Vec::new();
    out.extend_from_slice(b"FFNT");
    out.extend_from_slice(&[0xFF, 0xFE, 0x14, 0x00, 0x00, 0x00, 0x01, 0x04]);
    out.extend_from_slice(&[0; 4]);
    let mut blocks = 2u16;
    out.extend_from_slice(&[0; 4]);

    // Font information: single bytes, a line height and a replacement character, then three offsets.
    out.extend_from_slice(b"FINF");
    le32(&mut out, 0x20);
    out.extend_from_slice(&data[info..info + 4]);
    le16(&mut out, be16(data, info + 4)?);
    le16(&mut out, be16(data, info + 6)?);
    out.extend_from_slice(&data[info + 8..info + 0x0C]);
    let offsets = out.len();
    out.extend_from_slice(&[0; 12]);

    // Glyph sheets: the texture archive sits on a 4 KiB boundary, as in the game's own fonts.
    let tglp = out.len();
    out.extend_from_slice(b"TGLP");
    le32(&mut out, 0);
    set32(&mut out, offsets, tglp + 8);
    out.extend_from_slice(&data[sheets_at..sheets_at + 4]);
    le32(&mut out, bntx.len() as u32);
    le16(&mut out, be16(data, sheets_at + 8)?);
    le16(&mut out, format);
    for at in [0x0C, 0x0E, 0x10, 0x12] {
        le16(&mut out, be16(data, sheets_at + at)?);
    }
    le32(&mut out, 0x1000);
    out.resize(0x1000, 0);
    out.extend_from_slice(&bntx);
    out.resize(out.len().div_ceil(4) * 4, 0);
    let size = out.len() - tglp;
    set32(&mut out, tglp + 4, size);

    // Character widths: a range of glyphs, then three bytes each. A chain of sections.
    let mut link = offsets + 4;
    while widths_at != 0 {
        let at = section(data, widths_at, b"CWDH")?;
        let (first, last) = (be16(data, at)?, be16(data, at + 2)?);
        let start = out.len();
        out.extend_from_slice(b"CWDH");
        le32(&mut out, 0);
        set32(&mut out, link, start + 8);
        le16(&mut out, first);
        le16(&mut out, last);
        link = out.len();
        le32(&mut out, 0);
        let count = (last as usize + 1).checked_sub(first as usize).context("a width range is backwards")?;
        out.extend_from_slice(data.get(at + 8..at + 8 + 3 * count).context("the font's widths end early")?);
        out.resize(out.len().div_ceil(4) * 4, 0);
        let size = out.len() - start;
        set32(&mut out, start + 4, size);
        blocks += 1;
        widths_at = be32(data, at + 4)? as usize;
    }

    // Character maps: a range of codes and how to find their glyphs. Another chain.
    let mut link = offsets + 8;
    while map_at != 0 {
        let at = section(data, map_at, b"CMAP")?;
        let (first, last, method) = (be16(data, at)?, be16(data, at + 2)?, be16(data, at + 4)?);
        let start = out.len();
        out.extend_from_slice(b"CMAP");
        le32(&mut out, 0);
        set32(&mut out, link, start + 8);
        le32(&mut out, first as u32);
        le32(&mut out, last as u32);
        le16(&mut out, method);
        le16(&mut out, 0);
        link = out.len();
        le32(&mut out, 0);
        let body = at + 0x0C;
        match method {
            // Direct: the first glyph of the range.
            0 => le16(&mut out, be16(data, body)?),
            // Table: a glyph for every code of the range.
            1 => {
                let count = (last as usize + 1).checked_sub(first as usize).context("a code range is backwards")?;
                for i in 0..count {
                    le16(&mut out, be16(data, body + 2 * i)?);
                }
            }
            // Scan: pairs of code and glyph.
            2 => {
                let count = be16(data, body)?;
                le16(&mut out, count);
                le16(&mut out, 0);
                for i in 0..count as usize {
                    le32(&mut out, be16(data, body + 2 + 4 * i)? as u32);
                    le16(&mut out, be16(data, body + 4 + 4 * i)?);
                    le16(&mut out, 0);
                }
            }
            other => bail!("the font maps characters in a way this doesn't know ({other})"),
        }
        out.resize(out.len().div_ceil(4) * 4, 0);
        let size = out.len() - start;
        set32(&mut out, start + 4, size);
        blocks += 1;
        map_at = be32(data, at + 8)? as usize;
    }

    let size = out.len();
    set32(&mut out, 0x0C, size);
    out[0x10..0x12].copy_from_slice(&blocks.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A small version 3 font: one 32x32 sheet, two glyphs, three character maps.
    fn wii_u_font() -> Vec<u8> {
        let mut d = b"FFNT\xFE\xFF\x00\x14\x03\x00\x00\x00".to_vec();
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(b"FINF\x00\x00\x00\x20");
        d.extend_from_slice(&[1, 20, 18, 16, 0, 22, 0, 1, 0, 18, 18, 1]);
        d.extend_from_slice(&0x3Cu32.to_be_bytes());
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(b"TGLP\x00\x00\x00\x00");
        d.extend_from_slice(&[16, 16, 1, 16]);
        // A 32x32 BC4 sheet is 8x8 blocks, stored as 32x16 on the Wii U.
        d.extend_from_slice(&(32u32 * 16 * 8).to_be_bytes());
        d.extend_from_slice(&[0, 12, 0x00, 0x0C, 0, 2, 0, 2, 0, 32, 0, 32]);
        d.extend_from_slice(&0x100u32.to_be_bytes());
        d.resize(0x100, 0);
        d.extend((0..32 * 16 * 8).map(|i| (i / 8) as u8));
        let widths = d.len() + 8;
        d[0x2C..0x30].copy_from_slice(&(widths as u32).to_be_bytes());
        d.extend_from_slice(b"CWDH\x00\x00\x00\x18");
        d.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 0, 0]);
        let mut next = d.len() + 8;
        d[0x30..0x34].copy_from_slice(&(next as u32).to_be_bytes());
        for (first, last, method, body) in [
            (0x41u16, 0x42u16, 0u16, vec![0u8, 0, 0, 0]),
            (0x61, 0x62, 1, vec![0, 1, 0xFF, 0xFF]),
            (0, 0xFFFF, 2, vec![0, 1, 0x30, 0x42, 0, 1, 0, 0]),
        ] {
            let last_map = method == 2;
            next += 0x14 + body.len();
            d.extend_from_slice(b"CMAP");
            d.extend_from_slice(&(0x14 + body.len() as u32).to_be_bytes());
            d.extend_from_slice(&first.to_be_bytes());
            d.extend_from_slice(&last.to_be_bytes());
            d.extend_from_slice(&method.to_be_bytes());
            d.extend_from_slice(&[0; 2]);
            d.extend_from_slice(&(if last_map { 0 } else { next as u32 }).to_be_bytes());
            d.extend_from_slice(&body);
        }
        d
    }

    #[test]
    fn font_sections() {
        let out = convert_bffnt(&wii_u_font()).unwrap();
        let u32_at = |at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(&out[..0x0C], b"FFNT\xFF\xFE\x14\x00\x00\x00\x01\x04");
        assert_eq!(u32_at(0x0C), out.len());
        assert_eq!(out[0x10], 6);
        assert_eq!(&out[0x1C..0x28], &[1, 20, 18, 16, 22, 0, 1, 0, 0, 18, 18, 1]);
        assert_eq!(u32_at(0x28), 0x3C);
        assert_eq!(&out[0x1000..0x1004], b"BNTX");
        assert_eq!(u32_at(0x40), u32_at(0x1000 + 0x1C));
        assert_eq!(u32_at(0x50), 0x1000);
        let widths = u32_at(0x2C);
        assert_eq!(&out[widths - 8..widths - 4], b"CWDH");
        assert_eq!(&out[widths..widths + 14], &[0, 0, 1, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6]);
        let direct = u32_at(0x30);
        assert_eq!(&out[direct - 8..direct - 4], b"CMAP");
        assert_eq!((u32_at(direct), u32_at(direct + 4)), (0x41, 0x42));
        let table = u32_at(direct + 0x0C);
        assert_eq!(&out[table + 0x10..table + 0x14], &[1, 0, 0xFF, 0xFF]);
        let scan = u32_at(table + 0x0C);
        assert_eq!(u32_at(scan + 0x0C), 0);
        assert_eq!(&out[scan + 0x10..scan + 0x1C], &[1, 0, 0, 0, 0x42, 0x30, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn other_fonts_are_refused() {
        let mut font = wii_u_font();
        font[0x3C + 0x0B] = 0x05;
        assert!(convert_bffnt(&font).is_err());
        assert!(convert_bffnt(&font[..0x80]).is_err());
        assert!(convert_bffnt(b"FFNT\xFF\xFE").is_err());
    }
}
