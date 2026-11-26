//! BC1, the compression the map's tiles use: blocks of 4x4 pixels, each two colours and two bits a pixel.
//!
//! The encoder fits each block along its colours' main direction and then refines the two colours by least squares,
//! keeping whichever try comes closest to the cleaned image. Its arithmetic is single precision and in the order of
//! the Python tool it was written from (Savage Wind's `tools/maphd.py`), so both make the same blocks.

use std::sync::LazyLock;

/// Perceptual weights of the error, red, green and blue.
const WEIGHTS: [f32; 3] = [0.299f32 * 3.0, 0.587f32 * 3.0, 0.114f32 * 3.0];

type Pixels = [[f32; 3]; 16];

fn expand(c: u16) -> [f32; 3] {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [((r << 3) | (r >> 2)) as f32, ((g << 2) | (g >> 4)) as f32, ((b << 3) | (b >> 2)) as f32]
}

/// The four colours of a block: four of them when the first end is above the second, else three and black.
fn palette(c0: u16, c1: u16) -> [[f32; 3]; 4] {
    let (a, b) = (expand(c0), expand(c1));
    let four = c0 > c1;
    let mix = |i: usize, j: usize| -> [f32; 3] {
        std::array::from_fn(|c| {
            let (x, y) = ([a, b][i][c], [a, b][j][c]);
            if four { (2.0 * x + y) / 3.0 } else { (x + y) / 2.0 }
        })
    };
    let third = if four { mix(1, 0) } else { [0.0; 3] };
    [a, b, mix(0, 1), third]
}

/// Rows of blocks (8 bytes each) to plain colours, three bytes a pixel.
pub fn decode(blocks: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let bw = w.div_ceil(4);
    let mut out = vec![0u8; w * h * 3];
    for (n, block) in blocks.chunks_exact(8).enumerate().take(bw * h.div_ceil(4)) {
        let (bx, by) = (n % bw, n / bw);
        let c0 = u16::from_le_bytes([block[0], block[1]]);
        let c1 = u16::from_le_bytes([block[2], block[3]]);
        let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let pal = palette(c0, c1).map(|p| p.map(|v| v.round_ties_even() as u8));
        for i in 0..16 {
            let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
            if x < w && y < h {
                let at = (y * w + x) * 3;
                out[at..at + 3].copy_from_slice(&pal[((bits >> (2 * i)) & 3) as usize]);
            }
        }
    }
    out
}

fn to_565(c: [f32; 3]) -> u16 {
    let c = c.map(|v| v.round_ties_even().clamp(0.0, 255.0));
    let r = (c[0] * 31.0 / 255.0).round_ties_even() as u16;
    let g = (c[1] * 63.0 / 255.0).round_ties_even() as u16;
    let b = (c[2] * 31.0 / 255.0).round_ties_even() as u16;
    (r << 11) | (g << 5) | b
}

/// Sixteen values summed the way numpy sums a row: eight running sums, then pairs.
fn sum16(v: &[f32; 16]) -> f32 {
    let r: [f32; 8] = std::array::from_fn(|j| v[j] + v[j + 8]);
    ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]))
}

/// The indices and weighted error of a block's pixels against two ends.
fn fit(px: &Pixels, c0: u16, c1: u16) -> ([u8; 16], f32) {
    let pal = palette(c0, c1);
    let four = c0 > c1;
    let mut idx = [0u8; 16];
    let mut err = [0f32; 16];
    for (k, p) in px.iter().enumerate() {
        let mut best = (0u8, f32::INFINITY);
        for (m, q) in pal.iter().enumerate() {
            let d = (0..3).fold(0f32, |s, c| s + (p[c] - q[c]) * (p[c] - q[c]) * WEIGHTS[c]);
            // (index 3 is transparent in three-colour mode)
            let d = if m == 3 && !four { f32::INFINITY } else { d };
            if d < best.1 || m == 0 {
                best = (m as u8, d);
            }
        }
        (idx[k], err[k]) = best;
    }
    (idx, sum16(&err))
}

/// Least-squares ends for fixed indices (the weights along c0..c1 each index stands for).
fn refine(px: &Pixels, idx: &[u8; 16], four: bool) -> ([f32; 3], [f32; 3]) {
    let w4: [[f32; 2]; 4] = [[1.0, 0.0], [0.0, 1.0], [(2.0f64 / 3.0) as f32, (1.0f64 / 3.0) as f32], [
        (1.0f64 / 3.0) as f32,
        (2.0f64 / 3.0) as f32,
    ]];
    let w3: [[f32; 2]; 4] = [[1.0, 0.0], [0.0, 1.0], [0.5, 0.5], [0.0, 0.0]];
    let wt: [[f32; 2]; 16] = std::array::from_fn(|k| if four { w4 } else { w3 }[idx[k] as usize]);
    let mut ata = [[0f32; 2]; 2];
    let mut rhs = [[0f32; 3]; 2];
    for i in 0..2 {
        for j in 0..2 {
            ata[i][j] = wt.iter().fold(0f32, |s, w| s + w[i] * w[j]);
        }
        ata[i][i] += 1e-4f32;
        for c in 0..3 {
            rhs[i][c] = wt.iter().zip(px).fold(0f32, |s, (w, p)| s + w[i] * p[c]);
        }
    }
    // The 2x2 system solved in double precision, rounded once: what numpy's solve gives for these.
    let (mut a, mut b) = (ata.map(|r| r.map(f64::from)), rhs.map(|r| r.map(f64::from)));
    if a[1][0].abs() > a[0][0].abs() {
        a.swap(0, 1);
        b.swap(0, 1);
    }
    let l = a[1][0] / a[0][0];
    let u11 = a[1][1] - l * a[0][1];
    let mut x = [[0f32; 3]; 2];
    for c in 0..3 {
        let x1 = (b[1][c] - l * b[0][c]) / u11;
        let x0 = (b[0][c] - a[0][1] * x1) / a[0][0];
        (x[0][c], x[1][c]) = (x0 as f32, x1 as f32);
    }
    (x[0], x[1])
}

/// For each channel value 0..255, the 5- and 6-bit ends whose 2/3 mix comes closest, as close together as
/// possible (a flat colour stays flat however the GPU rounds).
static SINGLE: LazyLock<[Vec<[u16; 2]>; 2]> = LazyLock::new(|| {
    [5u32, 6].map(|bits| {
        let n = 1usize << bits;
        let x: Vec<f32> = (0..n as u32).map(|e| ((e << (8 - bits)) | (e >> (2 * bits - 8))) as f32).collect();
        (0..256)
            .map(|v| {
                let mut best = (0usize, f32::INFINITY);
                for i in 0..n {
                    for j in 0..n {
                        let mix = (2.0 * x[i] + x[j]) / 3.0;
                        let cost = (mix - v as f32).abs() * 1000.0 + (x[i] - x[j]).abs();
                        if cost < best.1 {
                            best = (i * n + j, cost);
                        }
                    }
                }
                [(best.0 / n) as u16, (best.0 % n) as u16]
            })
            .collect()
    })
});

fn single_colour(mean: [f32; 3]) -> (u16, u16) {
    let v = mean.map(|c| c.round_ties_even().clamp(0.0, 255.0) as usize);
    let (r, g, b) = (SINGLE[0][v[0]], SINGLE[1][v[1]], SINGLE[0][v[2]]);
    let c0 = (r[0] << 11) | (g[0] << 5) | b[0];
    let c1 = (r[1] << 11) | (g[1] << 5) | b[1];
    (c0.max(c1), c0.min(c1))
}

fn encode_block(px: &Pixels) -> [u8; 8] {
    // The main direction (power iteration on the covariance), ends at the extreme projections, inset a little.
    let mean: [f32; 3] = std::array::from_fn(|c| px.iter().fold(0f32, |s, p| s + p[c]) / 16.0);
    let cen: Pixels = px.map(|p| std::array::from_fn(|c| p[c] - mean[c]));
    let cov: [[f32; 3]; 3] =
        std::array::from_fn(|i| std::array::from_fn(|j| cen.iter().fold(0f32, |s, p| s + p[i] * p[j])));
    let mut v = [1f32; 3];
    for _ in 0..8 {
        let next: [f32; 3] = std::array::from_fn(|i| (0..3).fold(0f32, |s, j| s + cov[i][j] * v[j]));
        let norm = ((next[0] * next[0] + next[1] * next[1]) + next[2] * next[2]).sqrt() + 1e-9f32;
        v = next.map(|x| x / norm);
    }
    let t: [f32; 16] = std::array::from_fn(|k| (0..3).fold(0f32, |s, i| s + cen[k][i] * v[i]));
    let (lo, hi) = t.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &x| (lo.min(x), hi.max(x)));
    let inset = (hi - lo) / 32.0;
    let c0 = to_565(std::array::from_fn(|c| mean[c] + v[c] * (hi - inset)));
    let c1 = to_565(std::array::from_fn(|c| mean[c] + v[c] * (lo + inset)));
    let mut best = (c0.max(c1), c0.min(c1));
    let (mut best_idx, mut best_err) = fit(px, best.0, best.1);
    let mut keep = |c0: u16, c1: u16, best: &mut (u16, u16), best_idx: &mut [u8; 16]| {
        let (idx, err) = fit(px, c0, c1);
        if err < best_err {
            (*best, *best_idx, best_err) = ((c0, c1), idx, err);
        }
    };
    // (flat and nearly flat blocks keep their exact hue)
    let (s0, s1) = single_colour(mean);
    keep(s0, s1, &mut best, &mut best_idx);
    for _ in 0..2 {
        for four in [true, false] {
            let (a, b) = refine(px, &best_idx, four);
            let (c0, c1) = (to_565(a), to_565(b));
            let (hi, lo) = (c0.max(c1), c0.min(c1));
            let (c0, c1) = if four { (hi, lo) } else { (lo, hi) };
            keep(c0, c1, &mut best, &mut best_idx);
        }
    }
    let bits = best_idx.iter().enumerate().fold(0u32, |s, (k, &i)| s | (i as u32) << (2 * k));
    let mut out = [0u8; 8];
    out[..2].copy_from_slice(&best.0.to_le_bytes());
    out[2..4].copy_from_slice(&best.1.to_le_bytes());
    out[4..].copy_from_slice(&bits.to_le_bytes());
    out
}

/// Plain colours (three bytes a pixel) to rows of blocks. Past the right and bottom edges, the edge again.
pub fn encode(rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    let mut out = Vec::with_capacity(bw * bh * 8);
    for by in 0..bh {
        for bx in 0..bw {
            let px: Pixels = std::array::from_fn(|i| {
                let (x, y) = ((bx * 4 + i % 4).min(w - 1), (by * 4 + i / 4).min(h - 1));
                let at = (y * w + x) * 3;
                [rgb[at] as f32, rgb[at + 1] as f32, rgb[at + 2] as f32]
            });
            out.extend_from_slice(&encode_block(&px));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_colour_stays_flat_and_close() {
        let rgb = [90u8, 140, 60].repeat(64);
        let back = decode(&encode(&rgb, 8, 8), 8, 8);
        assert!(back.chunks(3).all(|p| p == &back[..3]));
        assert!(back[..3].iter().zip(&rgb).all(|(a, b)| a.abs_diff(*b) <= 4));
    }

    #[test]
    fn a_gradient_within_a_few_levels() {
        let rgb: Vec<u8> = (0..16 * 16).flat_map(|i| [(i % 16 * 16) as u8, 0, 0]).collect();
        let back = decode(&encode(&rgb, 16, 16), 16, 16);
        let diff: u32 = back.chunks(3).zip(rgb.chunks(3)).map(|(a, b)| a[0].abs_diff(b[0]) as u32).sum();
        assert!((diff as f32 / 256.0) < 6.0);
    }

    #[test]
    fn an_odd_size_repeats_its_edge() {
        let rgb: Vec<u8> = (0..7 * 5).flat_map(|i| [(i * 7) as u8, 50, 200]).collect();
        let blocks = encode(&rgb, 7, 5);
        assert_eq!(blocks.len(), 2 * 2 * 8);
        let back = decode(&blocks, 7, 5);
        assert_eq!(back.len(), rgb.len());
    }
}
