//! Smaller levels for a texture that came without them.
//!
//! A Wii U model's texture file holds each texture at full size only; the smaller copies the GPU uses at a distance
//! are in a second file that mods don't bring. On the Switch a texture carries its own, so a texture a mod changes
//! needs new ones: the texture is unpacked to plain colours, halved over and over, and each half is packed again.
//! The packing is a plain one (the two ends of each block's range of colours), which is enough at these sizes.

use super::tex::Kind;

/// Plain colours: four bytes a pixel, red first.
struct Image {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Image {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }

    /// The sixteen pixels of a block; past the edge, the edge again.
    fn block(&self, bx: u32, by: u32) -> [[u8; 4]; 16] {
        let mut out = [[0u8; 4]; 16];
        for (i, pixel) in out.iter_mut().enumerate() {
            *pixel = self.at(bx * 4 + i as u32 % 4, by * 4 + i as u32 / 4);
        }
        out
    }

    fn halved(&self) -> Image {
        let (width, height) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let four = [self.at(2 * x, 2 * y), self.at(2 * x + 1, 2 * y), self.at(2 * x, 2 * y + 1), self.at(
                    2 * x + 1,
                    2 * y + 1,
                )];
                for channel in 0..4 {
                    let sum: u32 = four.iter().map(|p| p[channel] as u32).sum();
                    pixels.push(((sum + 2) / 4) as u8);
                }
            }
        }
        Image { width, height, pixels }
    }
}

fn from_565(c: u16) -> [u8; 3] {
    let (r, g, b) = ((c >> 11) as u32, ((c >> 5) & 0x3F) as u32, (c & 0x1F) as u32);
    [((r * 527 + 23) >> 6) as u8, ((g * 259 + 33) >> 6) as u8, ((b * 527 + 23) >> 6) as u8]
}

fn to_565(c: [u8; 3]) -> u16 {
    let (r, g, b) = (c[0] as u32, c[1] as u32, c[2] as u32);
    ((((r * 249 + 1014) >> 11) << 11) | (((g * 253 + 505) >> 10) << 5) | ((b * 249 + 1014) >> 11)) as u16
}

/// The four colours of a colour block. `always_four` is how the formats with their own alpha read it.
fn colour_palette(c0: u16, c1: u16, always_four: bool) -> [[u8; 4]; 4] {
    let (a, b) = (from_565(c0), from_565(c1));
    let mix = |wa: u32, wb: u32, div: u32| -> [u8; 4] {
        let m = |i: usize| ((a[i] as u32 * wa + b[i] as u32 * wb + div / 2) / div) as u8;
        [m(0), m(1), m(2), 255]
    };
    let ends = ([a[0], a[1], a[2], 255], [b[0], b[1], b[2], 255]);
    if c0 > c1 || always_four {
        [ends.0, ends.1, mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [ends.0, ends.1, mix(1, 1, 2), [0, 0, 0, 0]]
    }
}

fn colour_block_out(block: &[u8], always_four: bool) -> [[u8; 4]; 16] {
    let (c0, c1) = (u16::from_le_bytes([block[0], block[1]]), u16::from_le_bytes([block[2], block[3]]));
    let palette = colour_palette(c0, c1, always_four);
    let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    std::array::from_fn(|i| palette[((indices >> (2 * i)) & 3) as usize])
}

/// Packs sixteen pixels' colours. `cutout` is the format whose blocks can have see-through pixels.
fn colour_block_in(pixels: &[[u8; 4]; 16], cutout: bool) -> [u8; 8] {
    let seen = |p: &[u8; 4]| !cutout || p[3] >= 128;
    let (mut low, mut high) = ([255u8; 3], [0u8; 3]);
    for pixel in pixels.iter().filter(|p| seen(p)) {
        for channel in 0..3 {
            low[channel] = low[channel].min(pixel[channel]);
            high[channel] = high[channel].max(pixel[channel]);
        }
    }
    let holes = pixels.iter().any(|p| !seen(p));
    if low[0] > high[0] {
        // Nothing to see in the whole block.
        (low, high) = ([0; 3], [0; 3]);
    }
    let (mut c0, mut c1) = (to_565(high), to_565(low));
    if holes {
        // Three colours and a hole: the first end isn't above the second.
        if c0 > c1 {
            std::mem::swap(&mut c0, &mut c1);
        }
    } else if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1);
    }
    let palette = colour_palette(c0, c1, !cutout);
    // With both ends the same the block reads as three colours and a hole: the hole is never picked.
    let usable = if holes || c0 == c1 { 3 } else { 4 };
    let mut indices = 0u32;
    for (i, pixel) in pixels.iter().enumerate() {
        let index = if !seen(pixel) {
            3
        } else {
            (0..usable)
                .min_by_key(|&k| {
                    (0..3).map(|c| (palette[k][c] as i32 - pixel[c] as i32).pow(2)).sum::<i32>()
                })
                .unwrap_or(0)
        };
        indices |= (index as u32) << (2 * i);
    }
    let mut out = [0u8; 8];
    out[..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..].copy_from_slice(&indices.to_le_bytes());
    out
}

fn grey_palette(a0: u8, a1: u8) -> [u8; 8] {
    let (a, b) = (a0 as u32, a1 as u32);
    let mut palette = [a0, a1, 0, 0, 0, 0, 0, 255];
    if a0 > a1 {
        for i in 1..7 {
            palette[i as usize + 1] = (((7 - i) * a + i * b + 3) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            palette[i as usize + 1] = (((5 - i) * a + i * b + 2) / 5) as u8;
        }
    }
    palette
}

/// One channel's block: two ends and sixteen three-bit picks between them.
fn grey_block_out(block: &[u8]) -> [u8; 16] {
    let palette = grey_palette(block[0], block[1]);
    let bits = block[2..8].iter().rev().fold(0u64, |all, &b| (all << 8) | b as u64);
    std::array::from_fn(|i| palette[((bits >> (3 * i)) & 7) as usize])
}

fn grey_block_in(values: &[u8; 16]) -> [u8; 8] {
    let (low, high) = (values.iter().copied().min().unwrap_or(0), values.iter().copied().max().unwrap_or(0));
    let mut out = [0u8; 8];
    out[0] = high;
    out[1] = low;
    if high == low {
        return out;
    }
    let palette = grey_palette(high, low);
    let mut bits = 0u64;
    for (i, &value) in values.iter().enumerate() {
        let index = (0..8).min_by_key(|&k| (palette[k] as i32 - value as i32).abs()).unwrap_or(0);
        bits |= (index as u64) << (3 * i);
    }
    out[2..].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}

/// How a format's elements hold colours.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Packing {
    Bytes(usize),
    /// Colour blocks, some pixels see-through.
    Colour,
    /// A grey block for the alpha and a colour block.
    AlphaColour,
    Grey,
    TwoGreys,
}

fn packing(kind: Kind) -> Option<Packing> {
    // The low byte says how the numbers read (plain or sRGB); the packing is the same.
    match (kind.nx_format >> 8, kind.nx_format & 0xFF) {
        (0x02, 1) => Some(Packing::Bytes(1)),
        (0x09, 1) => Some(Packing::Bytes(2)),
        (0x0B, 1 | 6) => Some(Packing::Bytes(4)),
        (0x1A, 1 | 6) => Some(Packing::Colour),
        (0x1C, 1 | 6) => Some(Packing::AlphaColour),
        (0x1D, 1) => Some(Packing::Grey),
        (0x1E, 1) => Some(Packing::TwoGreys),
        _ => None,
    }
}

fn unpack(packing: Packing, width: u32, height: u32, data: &[u8]) -> Option<Image> {
    let mut pixels = vec![255u8; (width * height * 4) as usize];
    match packing {
        Packing::Bytes(count) => {
            if data.len() != (width * height) as usize * count {
                return None;
            }
            for (pixel, from) in pixels.chunks_exact_mut(4).zip(data.chunks_exact(count)) {
                pixel[..count].copy_from_slice(from);
            }
        }
        _ => {
            let (blocks_w, blocks_h) = (width.div_ceil(4), height.div_ceil(4));
            let size = if matches!(packing, Packing::Colour | Packing::Grey) { 8 } else { 16 };
            if data.len() != (blocks_w * blocks_h) as usize * size {
                return None;
            }
            for (i, block) in data.chunks_exact(size).enumerate() {
                let colours: [[u8; 4]; 16] = match packing {
                    Packing::Colour => colour_block_out(block, false),
                    Packing::AlphaColour => {
                        let (alpha, mut colours) = (grey_block_out(block), colour_block_out(&block[8..], true));
                        for (colour, alpha) in colours.iter_mut().zip(alpha) {
                            colour[3] = alpha;
                        }
                        colours
                    }
                    Packing::Grey => grey_block_out(block).map(|v| [v, 0, 0, 255]),
                    _ => {
                        let (first, second) = (grey_block_out(block), grey_block_out(&block[8..]));
                        std::array::from_fn(|k| [first[k], second[k], 0, 255])
                    }
                };
                let (bx, by) = (i as u32 % blocks_w * 4, i as u32 / blocks_w * 4);
                for (k, colour) in colours.iter().enumerate() {
                    let (x, y) = (bx + k as u32 % 4, by + k as u32 / 4);
                    if x < width && y < height {
                        let at = ((y * width + x) * 4) as usize;
                        pixels[at..at + 4].copy_from_slice(colour);
                    }
                }
            }
        }
    }
    Some(Image { width, height, pixels })
}

fn pack(packing: Packing, image: &Image) -> Vec<u8> {
    if let Packing::Bytes(count) = packing {
        return image.pixels.chunks_exact(4).flat_map(|p| p[..count].to_vec()).collect();
    }
    let mut out = Vec::new();
    for by in 0..image.height.div_ceil(4) {
        for bx in 0..image.width.div_ceil(4) {
            let pixels = image.block(bx, by);
            match packing {
                Packing::Colour => out.extend_from_slice(&colour_block_in(&pixels, true)),
                Packing::AlphaColour => {
                    out.extend_from_slice(&grey_block_in(&pixels.map(|p| p[3])));
                    out.extend_from_slice(&colour_block_in(&pixels, false));
                }
                Packing::Grey => out.extend_from_slice(&grey_block_in(&pixels.map(|p| p[0]))),
                _ => {
                    out.extend_from_slice(&grey_block_in(&pixels.map(|p| p[0])));
                    out.extend_from_slice(&grey_block_in(&pixels.map(|p| p[1])));
                }
            }
        }
    }
    out
}

/// A level as plain colours, four bytes a pixel (for checks).
pub fn colours(kind: Kind, width: u32, height: u32, level: &[u8]) -> Option<Vec<u8>> {
    Some(unpack(packing(kind)?, width, height, level)?.pixels)
}

/// The `count` smaller levels of a texture, from its full-size one. `None` for formats this can't pack.
pub fn smaller_levels(kind: Kind, width: u32, height: u32, first: &[u8], count: usize) -> Option<Vec<Vec<u8>>> {
    let packing = packing(kind)?;
    let mut image = unpack(packing, width, height, first)?;
    let mut levels = Vec::with_capacity(count);
    for _ in 0..count {
        image = image.halved();
        levels.push(pack(packing, &image));
    }
    Some(levels)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::wiiu::tex::gx2_kind;

    fn kind(format: u32) -> Kind {
        gx2_kind(format, [0, 1, 2, 3]).unwrap()
    }

    /// A smooth picture: what the plain packing keeps well.
    fn picture(width: u32, height: u32) -> Image {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[(x * 255 / width) as u8, (y * 255 / height) as u8, 128, 255]);
            }
        }
        Image { width, height, pixels }
    }

    fn worst(a: &Image, b: &Image, channels: usize) -> i32 {
        a.pixels
            .chunks(4)
            .zip(b.pixels.chunks(4))
            .flat_map(|(p, q)| (0..channels).map(move |c| (p[c] as i32 - q[c] as i32).abs()))
            .max()
            .unwrap()
    }

    #[test]
    fn packing_and_unpacking_agree() {
        let image = picture(64, 32);
        for (format, channels, allowed) in [(0x431, 3, 20), (0x433, 4, 20), (0x034, 1, 3), (0x035, 2, 3), (0x41A, 4, 0)] {
            let packing = packing(kind(format)).unwrap();
            let packed = pack(packing, &image);
            let back = unpack(packing, 64, 32, &packed).unwrap();
            let off = worst(&image, &back, channels);
            assert!(off <= allowed, "format {format:#x} is off by {off}");
        }
    }

    #[test]
    fn see_through_pixels_stay_see_through() {
        let mut image = picture(8, 8);
        image.pixels[3] = 0;
        let packed = pack(Packing::Colour, &image);
        let back = unpack(Packing::Colour, 8, 8, &packed).unwrap();
        assert_eq!(back.pixels[3], 0);
        assert!(back.pixels.chunks(4).skip(1).all(|p| p[3] == 255));
    }

    #[test]
    fn levels_halve_down_to_one_pixel() {
        let first = pack(Packing::Colour, &picture(32, 8));
        let levels = smaller_levels(kind(0x431), 32, 8, &first, 5).unwrap();
        // 16x4, 8x2, 4x1, 2x1, 1x1: in blocks, 4, 2, 1, 1, 1.
        assert_eq!(levels.iter().map(Vec::len).collect::<Vec<_>>(), [32, 16, 8, 8, 8]);
        // The middle of the picture is still the middle colour.
        let last = unpack(Packing::Colour, 1, 1, &levels[4]).unwrap();
        assert!((last.pixels[0] as i32 - 123).abs() < 12 && (last.pixels[2] as i32 - 128).abs() < 12);
        assert!(smaller_levels(kind(0x234), 8, 8, &[0; 32], 1).is_none());
        assert!(smaller_levels(kind(0x431), 32, 8, &first[1..], 1).is_none());
    }
}
