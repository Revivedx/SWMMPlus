//! Synthetic map tiles (no game files and no Real-ESRGAN needed): small model files made here the way the game's
//! tiles are laid out, and stand-in "cleaned" images in place of Real-ESRGAN's.

use super::*;
use crate::wiiu::tex::{tegra_swizzle_at, tegra_unswizzle_at};

fn put32(d: &mut [u8], at: usize, v: usize) {
    d[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
}

fn put64(d: &mut [u8], at: usize, v: usize) {
    d[at..at + 8].copy_from_slice(&(v as u64).to_le_bytes());
}

/// A map tile of an image, laid out like the game's: the BNTX at 0x1000, its data after a BRTD header, each
/// part's relocation table after it.
fn make_tile(img: &Rgb, th: u32) -> Vec<u8> {
    let (w, h) = (img.width, img.height);
    let data = tegra_swizzle_at(&bc1::encode(&img.pixels, w, h), w.div_ceil(4), h.div_ceil(4), 8, th);
    let size = data.len();
    let (brti, brtd) = (0x200, 0xFF0);
    let mut head = vec![0u8; brtd + 0x10];
    head[..4].copy_from_slice(b"BNTX");
    put32(&mut head, 0x24, 1);
    put64(&mut head, 0x30, brtd);
    head[brti..brti + 4].copy_from_slice(b"BRTI");
    head[brti + 0x16] = 1;
    put32(&mut head, brti + 0x1C, tile::BC1_SRGB as usize);
    put32(&mut head, brti + 0x24, w as usize);
    put32(&mut head, brti + 0x28, h as usize);
    put32(&mut head, brti + 0x34, th as usize);
    put32(&mut head, brti + 0x50, size);
    head[brtd..brtd + 4].copy_from_slice(b"BRTD");
    put64(&mut head, brtd + 8, size + 0x10);
    let mut rlt = vec![0u8; 0x68];
    rlt[..4].copy_from_slice(b"_RLT");
    put32(&mut rlt, 4, brtd + 0x10 + size);
    put32(&mut rlt, 0x34, size + 0x10);
    put32(&mut head, 0x18, brtd + 0x10 + size);
    put32(&mut head, 0x1C, brtd + 0x10 + size + rlt.len());
    let bntx = [head, data, rlt].concat();
    let b = 0x1000;
    let f_rlt = (b + bntx.len() + 0xFFF) & !0xFFF;
    let mut frlt = vec![0u8; 0x10 + 0x18];
    frlt[..4].copy_from_slice(b"_RLT");
    put32(&mut frlt, 4, f_rlt);
    put32(&mut frlt, 8, 1);
    put32(&mut frlt, 0x18, b);
    put32(&mut frlt, 0x1C, f_rlt - b);
    put32(&mut frlt, 0x24, 1);
    let mut out = vec![0u8; b];
    out.extend_from_slice(&bntx);
    out.resize(f_rlt, 0);
    out.extend_from_slice(&frlt);
    out[..4].copy_from_slice(b"FRES");
    let len = out.len();
    put32(&mut out, 0x18, f_rlt);
    put32(&mut out, 0x1C, len);
    put64(&mut out, 0xF0, b);
    put64(&mut out, 0xF8, bntx.len());
    out
}

/// Blocky noise: random colours in 4x4 squares.
fn noise(h: u32, w: u32, seed: u64) -> Rgb {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 24) as u8
    };
    let (cw, ch) = (w / 4 + 1, h / 4 + 1);
    let cells: Vec<[u8; 3]> = (0..cw * ch).map(|_| [next(), next(), next()]).collect();
    let pixels = (0..h * w).flat_map(|i| cells[((i / w / 4) * cw + i % w / 4) as usize]).collect();
    Rgb { width: w, height: h, pixels }
}

fn mean_diff(a: &Rgb, b: &Rgb) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.pixels.iter().zip(&b.pixels).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.pixels.len() as f64
}

fn decoded(path: &Path) -> (tile::Tile, Rgb) {
    let (d, t) = tile::read(&fs::read(path).unwrap()).unwrap();
    let img = tile::rgb(&d, &t).unwrap();
    (t, img)
}

fn nearest_2x(img: &Rgb) -> Rgb {
    let (w, h) = (img.width * 2, img.height * 2);
    let pixels = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w / 2, i / w / 2);
            let at = ((y * img.width + x) * 3) as usize;
            [img.pixels[at], img.pixels[at + 1], img.pixels[at + 2]]
        })
        .collect();
    Rgb { width: w, height: h, pixels }
}

#[test]
fn yaz0_store_reads_back() {
    // (roead reads nothing shorter than 22 bytes)
    for d in [b"abcdefgh".to_vec(), b"abcdefghijklm".to_vec(), (0..=255u8).cycle().take(256 * 9).collect()] {
        assert_eq!(roead::yaz0::decompress(yaz0_store(&d)).unwrap(), d);
    }
}

#[test]
fn block_linear_round_trip() {
    for (w, h) in [(40u32, 40u32), (132, 132), (300, 300)] {
        let lin: Vec<u8> = (0..w.div_ceil(4) * h.div_ceil(4) * 8).map(|i| (i * 7 + i / 13) as u8).collect();
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let back = tegra_unswizzle_at(&tegra_swizzle_at(&lin, bw, bh, 8, 4), bw, bh, 8, 4).unwrap();
        assert_eq!(back, lin, "{w}x{h}");
    }
}

#[test]
fn tile_names() {
    let p = |level, column, row| Some(Position { level, column, row });
    assert_eq!(position("MapTex_A-1.sbmaptex"), p(0, b'A', 1));
    assert_eq!(position("MapTex3_Z-9.sbmaptex"), p(3, b'Z', 9));
    assert_eq!(position("MapTex_A-4_16384.sbmaptex"), p(0, b'A', 4));
    assert_eq!(position("MapTex1_K-0-x.sbmaptex"), p(1, b'K', 0));
    assert_eq!(position("MapTex_A-1x.sbmaptex"), None);
    assert_eq!(position("MapTex_a-1.sbmaptex"), None);
    assert_eq!(position("MapTex_A-.sbmaptex"), None);
    assert_eq!(position("MapTex_A-1.bfres"), None);
}

#[test]
fn a_tile_reads_back_and_takes_a_new_texture() {
    let img = noise(40, 40, 1);
    let file = make_tile(&img, 4);
    let t = tile::parse(&file).unwrap();
    assert_eq!((t.width, t.height, t.block_height), (40, 40, 4));
    assert!(mean_diff(&tile::rgb(&file, &t).unwrap(), &img) < 8.0);

    let flipped = Rgb { pixels: img.pixels.chunks(40 * 3).rev().flatten().copied().collect(), ..img.clone() };
    let same = tile::with(&file, &t, &flipped).unwrap();
    assert_eq!(same.len(), file.len(), "the same size keeps the file's length");
    assert!(mean_diff(&tile::rgb(&same, &tile::parse(&same).unwrap()).unwrap(), &flipped) < 8.0);
    assert_eq!(same[..t.data], file[..t.data]);

    let big_img = nearest_2x(&img);
    let big = tile::with(&file, &t, &big_img).unwrap();
    let bt = tile::parse(&big).unwrap();
    assert_eq!((bt.width, bt.height), (80, 80));
    assert!(mean_diff(&tile::rgb(&big, &bt).unwrap(), &big_img) < 8.0);
    // The model file around it grows: its relocation table, its size, its entry for the texture archive.
    let u32_at = |at: usize| u32::from_le_bytes(big[at..at + 4].try_into().unwrap()) as usize;
    let (f_rlt, size, b_size) = (u32_at(0x18), u32_at(0x1C), u32_at(0xF8));
    assert_eq!(size, big.len());
    assert_eq!(&big[f_rlt..f_rlt + 4], b"_RLT");
    assert_eq!(u32_at(f_rlt + 4), f_rlt);
    assert_eq!(f_rlt % 0x1000, 0);
    assert_eq!(u32_at(f_rlt + 0x10 + 12), f_rlt - 0x1000);
    assert_eq!(&big[0x1000 + b_size - 0x68..0x1000 + b_size - 0x64], b"_RLT");
}

#[test]
fn the_steps_on_a_corner_of_the_grid() {
    let t = tempfile::tempdir().unwrap();
    let (src, work) = (t.path().join("src"), t.path().join("work"));
    fs::create_dir_all(&src).unwrap();
    // A 2 x 2 corner of the grid at every level and a variant of one tile, sizes as 3000 / 840 / 300 / 135 go.
    let sizes = [40u32, 24, 16, 12];
    let mut images = std::collections::BTreeMap::new();
    for (level, &s) in sizes.iter().enumerate() {
        let digit = if level > 0 { level.to_string() } else { String::new() };
        for (k, (c, r)) in [('Z', 0), ('A', 0), ('Z', 1), ('A', 1)].into_iter().enumerate() {
            images.insert(format!("MapTex{digit}_{c}-{r}.sbmaptex"), noise(s, s, (level * 10 + k) as u64));
        }
        images.insert(format!("MapTex{digit}_A-0_4.sbmaptex"), noise(s, s, 99 + level as u64));
    }
    for (name, img) in &images {
        fs::write(src.join(name), yaz0_store(&make_tile(img, 4))).unwrap();
    }
    let run = |out: &Path, scale| Run {
        src: src.clone(),
        work: work.clone(),
        out: out.to_path_buf(),
        scale,
        pads: [8, 4, 4, 2],
        closest_size: OnceLock::new(),
    };
    let r = run(&t.path().join("out"), 1);

    r.pad_one("MapTex_A-0.sbmaptex").unwrap();
    let padded = Rgb::read_png(&work.join("L0/in/MapTex_A-0.png")).unwrap();
    let tile = |name: &str| decoded(&src.join(name)).1;
    let (me, left, below) = (tile("MapTex_A-0.sbmaptex"), tile("MapTex_Z-0.sbmaptex"), tile("MapTex_A-1.sbmaptex"));
    assert_eq!((padded.width, padded.height), (56, 56));
    assert_eq!(padded.crop(8, 8, 40, 40), me, "the tile in the middle");
    assert_eq!(padded.crop(0, 8, 8, 40), left.crop(32, 0, 8, 40), "the left border from the neighbour");
    assert_eq!(padded.crop(8, 48, 40, 8), below.crop(0, 0, 40, 8), "the bottom border from the neighbour");
    let first_row = me.crop(0, 0, 40, 1);
    for y in 0..8 {
        assert_eq!(padded.crop(8, y, 40, 1), first_row, "edge copies off the grid (row -1)");
    }

    // Stand-ins for Real-ESRGAN's images: the padded image enlarged 2x.
    for name in images.keys() {
        r.pad_one(name).unwrap();
        let level = position(name).unwrap().level;
        let stem = name.trim_end_matches(".sbmaptex");
        let a = Rgb::read_png(&work.join(format!("L{level}/in/{stem}.png"))).unwrap();
        fs::create_dir_all(work.join(format!("L{level}/x2"))).unwrap();
        nearest_2x(&a).write_png(&work.join(format!("L{level}/x2/{stem}.png"))).unwrap();
    }
    for scale in [1, 2] {
        let out = t.path().join(format!("out{scale}"));
        fs::create_dir_all(&out).unwrap();
        let r = run(&out, scale);
        for name in images.keys() {
            r.encode_one(name).unwrap();
        }
        for name in images.keys() {
            let level = position(name).unwrap().level;
            let want = sizes[level] * if scale == 2 && level < 2 { 2 } else { 1 };
            let (t, _) = decoded(&out.join(name));
            assert_eq!((t.width, t.height), (want, want), "scale {scale}: {name}");
        }
        let name = "MapTex2_A-1.sbmaptex";
        assert!(mean_diff(&decoded(&out.join(name)).1, &images[name]) < 8.0, "a tile comes back as the cleaned image");
    }
    // Scale 2: the medium level is reduced from the closest one (variants too).
    let (_, medium) = decoded(&t.path().join("out2/MapTex1_A-0_4.sbmaptex"));
    let from_closest = lanczos::resize(&images["MapTex_A-0_4.sbmaptex"], 48, 48);
    let from_own = nearest_2x(&images["MapTex1_A-0_4.sbmaptex"]);
    assert!(mean_diff(&medium, &from_closest) < mean_diff(&medium, &from_own));

    // Without the closest level's image, as with scale 1.
    fs::remove_file(work.join("L0/x2/MapTex_Z-1.png")).unwrap();
    let out = t.path().join("fallback");
    fs::create_dir_all(&out).unwrap();
    run(&out, 2).encode_one("MapTex1_Z-1.sbmaptex").unwrap();
    assert_eq!(decoded(&out.join("MapTex1_Z-1.sbmaptex")).0.width, sizes[1]);
}

/// A synthetic image whose pixels are a formula of their place.
fn pattern(w: u32, h: u32) -> Rgb {
    let pixels = (0..h)
        .flat_map(|y| (0..w).flat_map(move |x| (0..3).map(move |c| ((x * 37 + y * 91 + c * 53 + x * y * 7) % 256) as u8)))
        .collect();
    Rgb { width: w, height: h, pixels }
}

fn fnv(d: &[u8]) -> u64 {
    d.iter().fold(0xCBF2_9CE4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01B3))
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

// The expected bytes are Pillow's (`Image.resize(size, Image.LANCZOS)`) and the Python tool's encoder's, on the
// same synthetic images.
#[test]
fn lanczos_gives_pillows_pixels() {
    let small = lanczos::resize(&pattern(13, 11), 6, 5);
    assert_eq!(
        small.pixels,
        hex("5e818e7784977f89886e788a737f8976976d81877b8f87898f897e837f7d8585848691946ba17f7984689a807b7f9079867c608b6c8b\
             7d658b847d977d787c827d7e7c768a865a9b9a7981ae6a815d9e6d8d6f9f78a06f7f779e")
    );
    assert_eq!(fnv(&lanczos::resize(&pattern(13, 11), 20, 17).pixels), 0xA5DB_7399_16F7_1736);
    assert_eq!(fnv(&lanczos::resize(&pattern(300, 200), 150, 100).pixels), 0x3F05_0DEA_F28D_307A);
}

#[test]
fn bc1_gives_the_python_tools_blocks() {
    let img = pattern(12, 8);
    assert_eq!(
        bc1::encode(&img.pixels, 12, 8),
        hex("2dbfd331b5cbdc2dd28e4e69502dd22dcae0d2645571155c33526baf06611882abc8d26dc55cc51ceae0b16445175145")
    );
    let img = pattern(64, 40);
    assert_eq!(fnv(&bc1::encode(&img.pixels, 64, 40)), 0x9BCB_76BD_1514_9228);
}
