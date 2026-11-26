//! A map tile: a model file (BFRES) holding one texture archive (BNTX) with one BC1 texture, no smaller levels.
//!
//! A tile of the same size keeps its file as it is but for the texture's bytes. A bigger one grows the texture
//! archive and, with it, the model file around it: its entry for the embedded archive, the relocation table's
//! section over the archive and the file's size. Everything else keeps the game's bytes.

use anyhow_ext::{bail, Context, Result};

use super::{bc1, Rgb};
use crate::wiiu::tex::{tegra_swizzle_at, tegra_unswizzle_at};

pub const BC1_SRGB: u32 = 0x1A06;
pub const BC1_UNORM: u32 = 0x1A01;

/// What a tile's texture is and where its bytes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile {
    pub width: u32,
    pub height: u32,
    /// The block height the texture is stored with, as a power of two.
    pub block_height: u32,
    pub size: usize,
    /// Where the texture's bytes start in the file.
    pub data: usize,
    pub format: u32,
}

fn find(d: &[u8], magic: &[u8; 4], from: usize) -> Option<usize> {
    d.get(from..)?.windows(4).position(|w| w == magic).map(|at| at + from)
}

fn u16_at(d: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(d.get(at..at + 2).context("map tile: ends early")?.try_into()?))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(d.get(at..at + 4).context("map tile: ends early")?.try_into()?))
}

fn u64_at(d: &[u8], at: usize) -> Result<usize> {
    Ok(u64::from_le_bytes(d.get(at..at + 8).context("map tile: ends early")?.try_into()?) as usize)
}

fn put32(d: &mut [u8], at: usize, v: usize) {
    d[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
}

fn put64(d: &mut [u8], at: usize, v: usize) {
    d[at..at + 8].copy_from_slice(&(v as u64).to_le_bytes());
}

/// The bytes a BC1 texture takes in the Switch's storage order.
fn stored_size(width: u32, height: u32, block_height: u32) -> usize {
    let (row_bytes, rows) = (width.div_ceil(4) as usize * 8, height.div_ceil(4) as usize);
    let rows_per_block = 8usize << block_height;
    row_bytes.div_ceil(64) * 64 * rows.div_ceil(rows_per_block) * rows_per_block
}

/// A tile file, out of its Yaz0.
pub fn read(raw: &[u8]) -> Result<(Vec<u8>, Tile)> {
    let d = if raw.starts_with(b"Yaz0") { roead::yaz0::decompress(raw)? } else { raw.to_vec() };
    let tile = parse(&d)?;
    Ok((d, tile))
}

pub fn parse(d: &[u8]) -> Result<Tile> {
    let b = find(d, b"BNTX", 0).context("not a map tile (one BNTX texture expected)")?;
    if u32_at(d, b + 0x24)? != 1 {
        bail!("not a map tile (one BNTX texture expected)");
    }
    let r = find(d, b"BRTI", b).context("not a map tile (no texture in its BNTX)")?;
    let mips = u16_at(d, r + 0x16)?;
    let format = u32_at(d, r + 0x1C)?;
    let (width, height) = (u32_at(d, r + 0x24)?, u32_at(d, r + 0x28)?);
    let block_height = u32_at(d, r + 0x34)?;
    let size = u32_at(d, r + 0x50)? as usize;
    let brtd = u64_at(d, b + 0x30)?;
    if mips != 1 || ![BC1_SRGB, BC1_UNORM].contains(&format) || d.get(b + brtd..b + brtd + 4) != Some(b"BRTD") {
        bail!("not a map tile (mips {mips}, format {format:#x})");
    }
    if block_height > 4 || stored_size(width, height, block_height) != size {
        bail!("map tile: {width}x{height} block height {block_height} does not take {size} bytes");
    }
    if d.len() < b + brtd + 0x10 + size {
        bail!("map tile: ends early");
    }
    Ok(Tile { width, height, block_height, size, data: b + brtd + 0x10, format })
}

/// The tile's texture as plain colours.
pub fn rgb(d: &[u8], tile: &Tile) -> Result<Rgb> {
    let raw = &d[tile.data..tile.data + tile.size];
    let blocks = tegra_unswizzle_at(raw, tile.width.div_ceil(4), tile.height.div_ceil(4), 8, tile.block_height)
        .context("map tile: the texture ends early")?;
    Ok(Rgb { width: tile.width, height: tile.height, pixels: bc1::decode(&blocks, tile.width, tile.height) })
}

/// The tile with its texture replaced by `img`, compressed to BC1.
pub fn with(d: &[u8], tile: &Tile, img: &Rgb) -> Result<Vec<u8>> {
    let (w, h) = (img.width, img.height);
    let blocks = bc1::encode(&img.pixels, w, h);
    let data = tegra_swizzle_at(&blocks, w.div_ceil(4), h.div_ceil(4), 8, tile.block_height);
    if (w, h) == (tile.width, tile.height) {
        let mut out = d.to_vec();
        out[tile.data..tile.data + tile.size].copy_from_slice(&data);
        return Ok(out);
    }
    let b = find(d, b"BNTX", 0).context("not a map tile")?;
    let bntx_size = u32_at(d, b + 0x1C)? as usize;
    let x = d.get(b..b + bntx_size).context("map tile: the BNTX ends early")?;
    let brti = find(x, b"BRTI", 0).context("not a map tile")?;
    let brtd = u64_at(x, 0x30)?;
    let rlt_old = u32_at(x, 0x18)? as usize;
    if rlt_old != brtd + 0x10 + tile.size {
        bail!("map tile: the BNTX is not laid out as expected");
    }
    let mut rlt = x[rlt_old..].to_vec();
    let mut head = x[..brtd + 0x10].to_vec();
    put32(&mut head, brti + 0x24, w as usize);
    put32(&mut head, brti + 0x28, h as usize);
    put32(&mut head, brti + 0x50, data.len());
    put64(&mut head, brtd + 8, data.len() + 0x10);
    let new_rlt = brtd + 0x10 + data.len();
    put32(&mut head, 0x18, new_rlt);
    put32(&mut head, 0x1C, new_rlt + rlt.len());
    if rlt.len() < 0x38 {
        bail!("map tile: the BNTX relocation table is not laid out as expected");
    }
    put32(&mut rlt, 4, new_rlt);
    if u32_at(&rlt, 0x34)? as usize != tile.size + 0x10 {
        bail!("map tile: the BNTX relocation table is not laid out as expected");
    }
    put32(&mut rlt, 0x34, data.len() + 0x10);
    let data_len = data.len();
    let nb = [head, data, rlt].concat();
    let f_rlt = u32_at(d, 0x18)? as usize;
    if (u64_at(d, 0xF0)?, u64_at(d, 0xF8)?) != (b, bntx_size) || f_rlt != (b + bntx_size + 0xFFF) & !0xFFF {
        bail!("map tile: the BFRES is not laid out as expected");
    }
    let mut frlt = d.get(f_rlt..).context("map tile: ends early")?.to_vec();
    let new_f_rlt = (b + nb.len() + 0xFFF) & !0xFFF;
    let sections = u32_at(&frlt, 8)? as usize;
    for k in 0..sections {
        let o = 0x10 + k * 0x18;
        let (off, size) = (u32_at(&frlt, o + 8)? as usize, u32_at(&frlt, o + 12)? as usize);
        if off == b && off + size == f_rlt {
            put32(&mut frlt, o + 12, new_f_rlt - b);
        }
    }
    put32(&mut frlt, 4, new_f_rlt);
    let mut out = d[..b].to_vec();
    out.extend_from_slice(&nb);
    out.resize(new_f_rlt, 0);
    out.extend_from_slice(&frlt);
    let len = out.len();
    put32(&mut out, 0x18, new_f_rlt);
    put32(&mut out, 0x1C, len);
    put64(&mut out, 0xF8, nb.len());
    let back = parse(&out)?;
    if (back.width, back.height, back.size) != (w, h, data_len) {
        bail!("map tile: the bigger tile does not read back");
    }
    Ok(out)
}
