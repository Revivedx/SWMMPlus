//! Wii U textures to the Switch's.
//!
//! The pixels (or compressed blocks) are the same on both; what differs is the order they are stored in. The Wii U's
//! GPU keeps a texture in tiles, the Switch's in its own block-linear order. `gx2_untile` puts a Wii U texture in
//! plain rows, `tegra_swizzle` puts plain rows in the Switch's order, and `build_bntx` wraps the result as the
//! Switch's texture archive.

use anyhow_ext::{bail, Context, Result};
use uk_content::layout::bntx::Bntx;

/// What a texture is made of once its storage order is out of the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kind {
    /// The Switch's format code.
    pub nx_format: u32,
    /// Where the sampler takes red, green, blue and alpha from (one byte each: 0 zero, 1 one, 2 red, 3 green,
    /// 4 blue, 5 alpha).
    pub channels: u32,
    /// Pixels per element side: 4 for block-compressed formats, 1 otherwise.
    pub block: u32,
    /// Bytes per element (a pixel or a compressed block).
    pub bytes: u32,
}

const RGBA: u32 = 0x0504_0302;
const ALPHA_ONLY: u32 = 0x0201_0101;
const LUMINANCE: u32 = 0x0102_0202;
const LUMINANCE_ALPHA: u32 = 0x0302_0202;

/// The format byte of a Wii U layout image (.bflim).
pub fn flim_kind(format: u8) -> Option<Kind> {
    let kind = |nx_format, channels, block, bytes| Some(Kind { nx_format, channels, block, bytes });
    match format {
        0x00 => kind(0x0201, LUMINANCE, 1, 1),
        0x01 => kind(0x0201, ALPHA_ONLY, 1, 1),
        0x03 => kind(0x0901, LUMINANCE_ALPHA, 1, 2),
        0x09 => kind(0x0B01, RGBA, 1, 4),
        0x14 => kind(0x0B06, RGBA, 1, 4),
        0x0C => kind(0x1A01, RGBA, 4, 8),
        0x15 => kind(0x1A06, RGBA, 4, 8),
        0x0D => kind(0x1B01, RGBA, 4, 16),
        0x16 => kind(0x1B06, RGBA, 4, 16),
        0x0E => kind(0x1C01, RGBA, 4, 16),
        0x17 => kind(0x1C06, RGBA, 4, 16),
        0x0F => kind(0x1D01, LUMINANCE, 4, 8),
        0x10 => kind(0x1D01, ALPHA_ONLY, 4, 8),
        0x11 => kind(0x1E01, LUMINANCE_ALPHA, 4, 16),
        _ => None,
    }
}

/// The format of a Wii U model texture (a GX2 surface format) and its channel order (one byte per output
/// channel: 0 red, 1 green, 2 blue, 3 alpha, 4 zero, 5 one).
pub fn gx2_kind(format: u32, channels: [u8; 4]) -> Option<Kind> {
    let (nx_format, block, bytes) = match format {
        0x001 => (0x0201, 1, 1),
        0x007 => (0x0901, 1, 2),
        0x01A => (0x0B01, 1, 4),
        0x41A => (0x0B06, 1, 4),
        0x031 => (0x1A01, 4, 8),
        0x431 => (0x1A06, 4, 8),
        0x032 => (0x1B01, 4, 16),
        0x432 => (0x1B06, 4, 16),
        0x033 => (0x1C01, 4, 16),
        0x433 => (0x1C06, 4, 16),
        0x034 => (0x1D01, 4, 8),
        0x234 => (0x1D02, 4, 8),
        0x035 => (0x1E01, 4, 16),
        0x235 => (0x1E02, 4, 16),
        _ => return None,
    };
    let mut selector = 0u32;
    for (i, channel) in channels.into_iter().enumerate() {
        let nx = match channel {
            0..=3 => channel as u32 + 2,
            4 => 0,
            5 => 1,
            _ => return None,
        };
        selector |= nx << (8 * i);
    }
    Some(Kind { nx_format, channels: selector, block, bytes })
}

/// BC4 with the alpha in its one channel: what both consoles' fonts use for their glyph sheets.
pub const FONT_SHEET: Kind = Kind { nx_format: 0x1D01, channels: ALPHA_ONLY, block: 4, bytes: 8 };

fn round_up(x: u32, to: u32) -> u32 {
    x.div_ceil(to) * to
}

fn micro_index(x: u32, y: u32, bits: u32) -> u32 {
    let (x0, x1, x2) = (x & 1, (x >> 1) & 1, (x >> 2) & 1);
    let (y0, y1, y2) = (y & 1, (y >> 1) & 1, (y >> 2) & 1);
    let order = match bits {
        8 => [x0, x1, x2, y1, y0, y2],
        16 => [x0, x1, x2, y0, y1, y2],
        64 => [x0, y0, x1, x2, y1, y2],
        128 => [y0, x0, x1, x2, y1, y2],
        _ => [x0, x1, y0, x2, y1, y2],
    };
    order.iter().enumerate().map(|(i, bit)| bit << i).sum()
}

/// A Wii U texture's elements in plain rows. `width` and `height` are in elements (blocks for compressed formats),
/// `tile_mode` and `swizzle` are the GX2 surface's (a .bflim keeps them in one byte), `slice` is the layer of an
/// array texture and `data` holds that layer onwards.
pub fn gx2_untile(
    data: &[u8],
    width: u32,
    height: u32,
    bytes: u32,
    tile_mode: u8,
    swizzle: u8,
    slice: u32,
) -> Result<Vec<u8>> {
    let bits = bytes * 8;
    // How wide and tall the surface is stored: whole tiles.
    let (pitch, stored_height) = match tile_mode {
        0 => (width, height),
        1 => (round_up(width, (2048 / bits).max(64)), height),
        2 => (round_up(width, 8), round_up(height, 8)),
        4 => (round_up(width, 32 * (32 / bits).max(1)), round_up(height, 16)),
        other => bail!("GX2 tile mode {other} isn't supported"),
    };
    let stored = (pitch * stored_height * bytes) as usize;
    if data.len() < stored {
        bail!(
            "the texture holds {} bytes and a {width}x{height} one in tile mode {tile_mode} needs {stored}",
            data.len()
        );
    }
    let (pipe_swizzle, bank_swizzle) = ((swizzle & 1) as u32, ((swizzle >> 1) & 3) as u32);
    let mut out = vec![0u8; (width * height * bytes) as usize];
    for y in 0..height {
        for x in 0..width {
            let address = match tile_mode {
                0 | 1 => (y * pitch + x) * bytes,
                2 => bytes * 64 * ((x >> 3) + (y >> 3) * (pitch >> 3)) + bytes * micro_index(x, y, bits),
                _ => {
                    let pipe = ((y >> 3) ^ (x >> 3)) & 1;
                    let bank = (((y >> 5) ^ (x >> 3)) & 1) | 2 * (((y >> 4) ^ (x >> 4)) & 1);
                    // Each layer of an array turns the bank and pipe by two.
                    let bank_pipe = ((pipe + 2 * bank) ^ (pipe_swizzle + 2 * bank_swizzle + 2 * slice)) % 8;
                    let (pipe, bank) = (bank_pipe % 2, bank_pipe / 2);
                    let macro_bytes = bytes * 16 * 32;
                    let macro_offset = ((x / 32) + (pitch / 32) * (y / 16)) * macro_bytes;
                    let total = bytes * micro_index(x, y, bits) + (macro_offset >> 3);
                    (bank << 9) | (pipe << 8) | (total & 255) | ((total & !255) << 3)
                }
            } as usize;
            let to = ((y * width + x) * bytes) as usize;
            let from = data.get(address..address + bytes as usize).context("texture data ends early")?;
            out[to..to + bytes as usize].copy_from_slice(from);
        }
    }
    Ok(out)
}

/// The Switch's block height for a texture `height` elements tall, as the power of two a texture header stores.
pub fn tegra_block_height_log2(height: u32) -> u32 {
    height.div_ceil(8).max(1).next_power_of_two().min(16).trailing_zeros()
}

fn tegra_address(x: u32, y: u32, width: u32, bytes: u32, block_height: u32) -> u32 {
    let gobs_per_row = (width * bytes).div_ceil(64);
    let gob = (y / (8 * block_height)) * 512 * block_height * gobs_per_row
        + (x * bytes / 64) * 512 * block_height
        + (y % (8 * block_height) / 8) * 512;
    let x = x * bytes;
    gob + ((x % 64) / 32) * 256 + ((y % 8) / 2) * 64 + ((x % 32) / 16) * 32 + (y % 2) * 16 + (x % 16)
}

#[cfg(test)]
fn tegra_size(width: u32, height: u32, bytes: u32) -> usize {
    tegra_size_at(width, height, bytes, 1 << tegra_block_height_log2(height))
}

fn tegra_size_at(width: u32, height: u32, bytes: u32, block_height: u32) -> usize {
    (round_up(width * bytes, 64) * round_up(height, block_height * 8)) as usize
}

/// Plain rows of elements in the Switch's storage order.
pub fn tegra_swizzle(linear: &[u8], width: u32, height: u32, bytes: u32) -> Vec<u8> {
    tegra_swizzle_at(linear, width, height, bytes, tegra_block_height_log2(height))
}

/// The same with a block height of its own (a texture that is replaced keeps the one its header says).
pub fn tegra_swizzle_at(linear: &[u8], width: u32, height: u32, bytes: u32, block_height_log2: u32) -> Vec<u8> {
    let block_height = 1 << block_height_log2.min(4);
    let mut out = vec![0u8; tegra_size_at(width, height, bytes, block_height)];
    for y in 0..height {
        for x in 0..width {
            let to = tegra_address(x, y, width, bytes, block_height) as usize;
            let from = ((y * width + x) * bytes) as usize;
            out[to..to + bytes as usize].copy_from_slice(&linear[from..from + bytes as usize]);
        }
    }
    out
}

/// The Switch's storage order back to plain rows (for tests and checks).
pub fn tegra_unswizzle(data: &[u8], width: u32, height: u32, bytes: u32) -> Option<Vec<u8>> {
    tegra_unswizzle_at(data, width, height, bytes, tegra_block_height_log2(height))
}

/// The same for a texture that says its own block height.
pub fn tegra_unswizzle_at(
    data: &[u8],
    width: u32,
    height: u32,
    bytes: u32,
    block_height_log2: u32,
) -> Option<Vec<u8>> {
    let block_height = 1 << block_height_log2.min(4);
    let mut out = vec![0u8; (width * height * bytes) as usize];
    for y in 0..height {
        for x in 0..width {
            let from = tegra_address(x, y, width, bytes, block_height) as usize;
            let to = ((y * width + x) * bytes) as usize;
            out[to..to + bytes as usize].copy_from_slice(data.get(from..from + bytes as usize)?);
        }
    }
    Some(out)
}

/// A texture ready for the Switch: plain rows of elements, one buffer per layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    pub name: String,
    /// In pixels.
    pub width: u32,
    pub height: u32,
    pub kind: Kind,
    pub layers: Vec<Vec<u8>>,
    /// The smaller levels of a one-layer texture, each half the one before.
    pub mips: Vec<Vec<u8>>,
    /// The Switch's fonts mark their sheets as an array even with one layer.
    pub array: bool,
}

/// A Wii U layout image: the pixels first, the header at the end of the file.
pub fn read_bflim(name: &str, data: &[u8]) -> Result<Texture> {
    let Some(start) = data.len().checked_sub(0x28).filter(|&s| &data[s..s + 4] == b"FLIM") else {
        bail!("not a layout image");
    };
    let header = &data[start..];
    if header[4..6] != [0xFE, 0xFF] {
        bail!("not a Wii U layout image");
    }
    let be16 = |o: usize| u16::from_be_bytes([header[o], header[o + 1]]) as u32;
    let (width, height) = (be16(0x1C), be16(0x1E));
    let (format, tiling) = (header[0x22], header[0x23]);
    let size = u32::from_be_bytes([header[0x24], header[0x25], header[0x26], header[0x27]]) as usize;
    let kind = flim_kind(format).with_context(|| format!("image format {format:#x} isn't supported"))?;
    let pixels = data.get(..size.min(start)).context("image data ends early")?;
    let linear = gx2_untile(
        pixels,
        width.div_ceil(kind.block),
        height.div_ceil(kind.block),
        kind.bytes,
        tiling & 0x1F,
        tiling >> 5,
        0,
    )?;
    Ok(Texture {
        name: name.strip_suffix(".bflim").unwrap_or(name).to_string(),
        width,
        height,
        kind,
        layers: vec![linear],
        mips: Vec::new(),
        array: false,
    })
}

const BRTI: usize = 0xA0;
const TEX_OBJECT: usize = 0x100;
const MEMORY_POOL: usize = 0x140;

/// The Switch's texture archive with `textures` in it.
///
/// The merger's own BNTX reader and writer do the real layout (string table, dictionary, relocations). What goes in
/// here is the smallest file that reader takes: the header fields and texture records it looks at, in any order.
pub fn build_bntx(file_name: &str, textures: &[Texture]) -> Result<Vec<u8>> {
    let mut out = vec![0u8; 0x58];
    out[..4].copy_from_slice(b"BNTX");
    // Version 0.4, little endian, 4 KiB alignment, target address size 0x40: what the game's own archives say.
    out[8..0x10].copy_from_slice(&[0, 0, 4, 0, 0xFF, 0xFE, 0x0C, 0x40]);
    out[0x20..0x24].copy_from_slice(b"NX  ");
    out[0x24..0x28].copy_from_slice(&(textures.len() as u32).to_le_bytes());
    let put64 = |out: &mut Vec<u8>, at: usize, value: usize| {
        out[at..at + 8].copy_from_slice(&(value as u64).to_le_bytes())
    };
    let string = |out: &mut Vec<u8>, text: &str| -> usize {
        let at = out.len();
        out.extend_from_slice(&(text.len() as u16).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out.push(0);
        out.resize(out.len().div_ceil(8) * 8, 0);
        at
    };
    let pool = out.len();
    put64(&mut out, 0x40, pool);
    out.resize(pool + MEMORY_POOL, 0);
    let name = string(&mut out, file_name);
    out[0x10..0x14].copy_from_slice(&(name as u32 + 2).to_le_bytes());
    let table = out.len();
    put64(&mut out, 0x28, table);
    out.resize(table + 8 * textures.len(), 0);
    for (i, texture) in textures.iter().enumerate() {
        let kind = texture.kind;
        let (width, height) = (texture.width.div_ceil(kind.block), texture.height.div_ceil(kind.block));
        if !texture.mips.is_empty() && texture.layers.len() != 1 {
            bail!("{}: smaller levels on a texture with {} layers", texture.name, texture.layers.len());
        }
        let mut data = Vec::new();
        for layer in &texture.layers {
            if layer.len() != (width * height * kind.bytes) as usize {
                bail!("{}: a layer holds {} bytes, not {width}x{height}", texture.name, layer.len());
            }
            data.extend_from_slice(&tegra_swizzle(layer, width, height, kind.bytes));
        }
        // Each smaller level follows the one before, in the block height its own size calls for.
        let mut level_starts = vec![0usize];
        for (i, level) in texture.mips.iter().enumerate() {
            let shift = i as u32 + 1;
            let (w, h) = (
                (texture.width >> shift).max(1).div_ceil(kind.block),
                (texture.height >> shift).max(1).div_ceil(kind.block),
            );
            if level.len() != (w * h * kind.bytes) as usize {
                bail!("{}: level {shift} holds {} bytes, not {w}x{h}", texture.name, level.len());
            }
            level_starts.push(data.len());
            data.extend_from_slice(&tegra_swizzle(level, w, h, kind.bytes));
        }
        let name = string(&mut out, &texture.name);
        let mips = out.len();
        out.resize(mips + 8 * level_starts.len(), 0);
        let pixels = out.len();
        for (i, start) in level_starts.iter().enumerate() {
            put64(&mut out, mips + 8 * i, pixels + start);
        }
        out.extend_from_slice(&data);
        out.resize(out.len().div_ceil(8) * 8, 0);
        let record = out.len();
        put64(&mut out, table + 8 * i, record);
        out.resize(record + BRTI + 2 * TEX_OBJECT, 0);
        out[record..record + 4].copy_from_slice(b"BRTI");
        let info = &mut out[record + 0x10..record + 0x60];
        info[0] = 1; // flags
        info[1] = 2; // two-dimensional storage
        info[6..8].copy_from_slice(&(1 + texture.mips.len() as u16).to_le_bytes()); // levels
        info[8..10].copy_from_slice(&1u16.to_le_bytes()); // one sample
        let fields: [(usize, u32); 11] = [
            (0x0C, kind.nx_format),
            (0x10, 0x20), // the GPU may sample it
            (0x14, texture.width),
            (0x18, texture.height),
            (0x1C, 1),
            (0x20, texture.layers.len() as u32),
            (0x24, tegra_block_height_log2(height)),
            (0x28, 0x0001_0007),
            (0x40, data.len() as u32),
            (0x44, 0x200),
            (0x48, kind.channels),
        ];
        for (at, value) in fields {
            info[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        info[0x4C] = if texture.array { 5 } else { 1 };
        put64(&mut out, record + 0x60, name);
        put64(&mut out, record + 0x70, mips);
        put64(&mut out, record + 0x80, record + BRTI);
        put64(&mut out, record + 0x88, record + BRTI + TEX_OBJECT);
    }
    Ok(Bntx::parse(&out).context("the texture archive didn't come out readable")?.to_binary())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn tegra_round_trip_and_sizes() {
        // Sizes and block heights the game's own layout atlas has for these shapes.
        assert_eq!(tegra_block_height_log2(46), 3);
        assert_eq!(tegra_block_height_log2(12), 1);
        assert_eq!(tegra_block_height_log2(32), 2);
        assert_eq!(tegra_block_height_log2(188), 4);
        assert_eq!(tegra_size(46, 46, 1), 4096);
        assert_eq!(tegra_size(12, 12, 8), 2048);
        assert_eq!(tegra_size(188, 188, 2), 98304);
        assert_eq!(tegra_size(64, 256, 8), 0x20000);
        let linear: Vec<u8> = (0..46 * 46).map(|i| (i * 7 % 251) as u8).collect();
        let stored = tegra_swizzle(&linear, 46, 46, 1);
        assert_eq!(tegra_unswizzle(&stored, 46, 46, 1).unwrap(), linear);
    }

    #[test]
    fn gx2_sizes_and_coverage() {
        // Every element of a tiled surface comes from its own place: tiling a marked surface and reading it back
        // gives each mark once. Sizes are the ones Wii U layout images have for these shapes.
        for (width, height, bytes, mode, stored) in [
            (48u32, 48u32, 1u32, 4u8, 6144usize),
            (12, 12, 16, 2, 4096),
            (29, 15, 16, 4, 8192),
            (47, 47, 8, 4, 24576),
            (240, 125, 8, 4, 262144),
            (32, 256, 8, 4, 65536),
        ] {
            let mut data = vec![0u8; stored];
            for (i, chunk) in data.chunks_mut(bytes as usize).enumerate() {
                chunk[0] = (i % 251) as u8;
                if bytes > 1 {
                    chunk[1] = (i / 251) as u8;
                }
            }
            for swizzle in [0u8, 3, 6] {
                let out = gx2_untile(&data, width, height, bytes, mode, swizzle, 0).unwrap();
                let mut seen = std::collections::HashSet::new();
                for chunk in out.chunks(bytes as usize) {
                    let mark = (chunk[0], if bytes > 1 { chunk[1] } else { 0 });
                    assert!(bytes == 1 || seen.insert(mark), "{width}x{height} mode {mode} reads a place twice");
                }
            }
            assert!(gx2_untile(&data[..stored - 1], width, height, bytes, mode, 0, 0).is_err());
        }
    }

    #[test]
    fn bntx_holds_the_textures() {
        let texture = Texture {
            name: "Icon^s".into(),
            width: 46,
            height: 46,
            kind: flim_kind(0x0E).unwrap(),
            layers: vec![(0..12 * 12 * 16).map(|i| i as u8).collect()],
            mips: Vec::new(),
            array: false,
        };
        let bntx = build_bntx("__Combined", &[texture]).unwrap();
        let parsed = Bntx::parse(&bntx).unwrap();
        assert_eq!(parsed.file_name, "__Combined");
        assert_eq!(parsed.textures.len(), 1);
        assert_eq!(parsed.textures[0].name, "Icon^s");
        assert_eq!(parsed.to_binary(), bntx);
    }
}
