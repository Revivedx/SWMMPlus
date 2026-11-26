//! Lanczos resizing, as Pillow does it for 8-bit images: a horizontal pass then a vertical one, each with weights
//! rounded to 22 bits and the result clamped to a byte in between. The Python tool resized with Pillow, so this
//! gives the same pixels.

use super::Rgb;

const PRECISION_BITS: u32 = 32 - 8 - 2;
const SUPPORT: f64 = 3.0;

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        return 1.0;
    }
    let x = x * std::f64::consts::PI;
    x.sin() / x
}

fn lanczos(x: f64) -> f64 {
    if (-SUPPORT..SUPPORT).contains(&x) { sinc(x) * sinc(x / 3.0) } else { 0.0 }
}

/// For each output position, the first input position it reads, and its weights.
fn coefficients(in_size: u32, out_size: u32) -> Vec<(usize, Vec<i32>)> {
    let scale = in_size as f64 / out_size as f64;
    let filter_scale = scale.max(1.0);
    let support = SUPPORT * filter_scale;
    let ss = 1.0 / filter_scale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let xmin = ((center - support + 0.5) as i64).max(0);
            let xmax = ((center + support + 0.5) as i64).min(in_size as i64);
            let weights: Vec<f64> =
                (xmin..xmax).map(|x| lanczos((x as f64 - center + 0.5) * ss)).collect();
            let total = weights.iter().fold(0.0, |s, w| s + w);
            let fixed = weights
                .iter()
                .map(|&w| {
                    let w = if total != 0.0 { w / total } else { w };
                    let w = w * (1 << PRECISION_BITS) as f64;
                    (if w < 0.0 { -0.5 + w } else { 0.5 + w }) as i32
                })
                .collect();
            (xmin as usize, fixed)
        })
        .collect()
}

fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

fn horizontal(img: &Rgb, width: u32) -> Rgb {
    let coeffs = coefficients(img.width, width);
    let mut pixels = Vec::with_capacity(width as usize * img.height as usize * 3);
    for row in img.pixels.chunks_exact(img.width as usize * 3) {
        for (xmin, k) in &coeffs {
            let mut ss = [1i32 << (PRECISION_BITS - 1); 3];
            for (x, &w) in k.iter().enumerate() {
                let at = (xmin + x) * 3;
                for c in 0..3 {
                    ss[c] += row[at + c] as i32 * w;
                }
            }
            pixels.extend(ss.map(clip8));
        }
    }
    Rgb { width, height: img.height, pixels }
}

fn vertical(img: &Rgb, height: u32) -> Rgb {
    let coeffs = coefficients(img.height, height);
    let stride = img.width as usize * 3;
    let mut pixels = Vec::with_capacity(stride * height as usize);
    let mut ss = vec![0i32; stride];
    for (ymin, k) in &coeffs {
        ss.fill(1 << (PRECISION_BITS - 1));
        for (y, &w) in k.iter().enumerate() {
            let row = &img.pixels[(ymin + y) * stride..][..stride];
            for (s, &p) in ss.iter_mut().zip(row) {
                *s += p as i32 * w;
            }
        }
        pixels.extend(ss.iter().map(|&s| clip8(s)));
    }
    Rgb { width: img.width, height, pixels }
}

/// The image at another size (Pillow's `Image.resize(size, Image.LANCZOS)`).
pub fn resize(img: &Rgb, width: u32, height: u32) -> Rgb {
    let img = if width != img.width { horizontal(img, width) } else { img.clone() };
    if height != img.height { vertical(&img, height) } else { img }
}
