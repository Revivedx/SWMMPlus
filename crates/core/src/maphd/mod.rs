//! A sharper world map, made from the player's own game files: every map tile cleaned with Real-ESRGAN, as a mod.
//!
//! The map's tiles (`UI/MapTex/MainField/MapTex{,1,2,3}_*.sbmaptex`: four zoom levels, 586 tiles each, BC1
//! textures) are decoded with a border taken from their neighbours, so the seams match; Real-ESRGAN
//! (`realesr-animevideov3`, 2x, the external `realesrgan-ncnn-vulkan`) cleans each level separately (the coarser
//! levels are drawn separately, with fewer contour lines, not reduced copies); each tile is then scaled back,
//! cropped and compressed to BC1 again, into the mod's `romfs/UI/MapTex/MainField/`.
//!
//! Scale 2: the game draws the two closest zoom levels at one tile texel per pixel of a 1080-line picture, so on a
//! 4K window each texel covers 2x2 pixels. Stored at twice the size, the closest level (3000 -> 6000 pixels a tile)
//! is Real-ESRGAN's image itself and the medium level (840 -> 1680) is reduced from it (the detailed drawing, not the
//! game's simpler one at that zoom); the two coarsest levels are as with scale 1. This only shows more detail if
//! the map is also drawn at the window's resolution (Savage Wind #1137); at 1080 lines the bigger tiles are reduced
//! back. The bigger tiles take about 11 GB more on disk.
//!
//! The in-between images go in a work folder (about 20 GB for the whole map); a stopped run goes on where it was.
//! The mod holds the player's map textures, changed, and must not be shared. Nothing is downloaded and nothing
//! outside the mod and work folders is written.
//!
//! This is a port of Savage Wind's `tools/maphd.py` and makes the same tiles from the same Real-ESRGAN images.

mod bc1;
mod lanczos;
mod tile;

use std::{
    io::BufWriter,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    },
    time::Instant,
};

use anyhow_ext::{bail, Context, Result};
use fs_err as fs;
use rayon::prelude::*;

/// Where the tiles are, in the game's files and in the mod.
pub const TILES: &str = "UI/MapTex/MainField";
/// The Real-ESRGAN model used (its `-x2` files must be in the `models` folder next to the program).
pub const MODEL: &str = "realesr-animevideov3";
/// The grid's columns, left to right (rows are 0..9 top to bottom).
const COLUMNS: &[u8] = b"ZABCDEFGHIJK";
/// The border taken from the neighbours, per level (pixels at the tile's own size).
const PADS: [u32; 4] = [64, 24, 12, 8];
/// Written in the mod folder, outside `romfs/` so the game doesn't see it: the scale the tiles were made at. (A
/// `rules.txt` next to it names the mod for the packager.)
const STAMP: &str = "maphd-scale.txt";

/// Plain colours, three bytes a pixel, row after row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgb {
    fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Rgb {
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for row in y..y + height {
            let at = ((row * self.width + x) * 3) as usize;
            pixels.extend_from_slice(&self.pixels[at..at + (width * 3) as usize]);
        }
        Rgb { width, height, pixels }
    }

    fn read_png(path: &Path) -> Result<Rgb> {
        let mut decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(path)?));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().with_context(|| format!("{}: not a PNG", path.display()))?;
        let mut buf = vec![0; reader.output_buffer_size().context("PNG too big")?];
        let info = reader.next_frame(&mut buf).with_context(|| format!("{}: can't be read", path.display()))?;
        buf.truncate(info.buffer_size());
        let pixels = match info.color_type {
            png::ColorType::Rgb => buf,
            png::ColorType::Rgba => buf.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect(),
            png::ColorType::Grayscale => buf.iter().flat_map(|&v| [v, v, v]).collect(),
            png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0]]).collect(),
            other => bail!("{}: PNG of type {other:?}", path.display()),
        };
        Ok(Rgb { width: info.width, height: info.height, pixels })
    }

    /// Written next to `path` first and then moved in place, so a stopped run leaves no half image.
    fn write_png(&self, path: &Path) -> Result<()> {
        let temp = path.with_extension("tmp.png");
        {
            let mut encoder = png::Encoder::new(BufWriter::new(fs::File::create(&temp)?), self.width, self.height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&self.pixels)?;
            writer.finish()?;
        }
        fs::rename(&temp, path)?;
        Ok(())
    }
}

/// Yaz0 without compression (every byte a literal): the game reads it like any other, and it is quick to make.
pub fn yaz0_store(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + d.len() + d.len().div_ceil(8));
    out.extend_from_slice(b"Yaz0");
    out.extend_from_slice(&(d.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0; 8]);
    for chunk in d.chunks(8) {
        out.push((0xFF00u32 >> chunk.len()) as u8);
        out.extend_from_slice(chunk);
    }
    out
}

/// A tile's file name: its level, column and row (`MapTex2_A-1.sbmaptex`, and variants like `MapTex_A-1_0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub level: usize,
    pub column: u8,
    pub row: u32,
}

/// The position of a tile from its file name, or `None` for anything else.
pub fn position(name: &str) -> Option<Position> {
    let rest = name.strip_prefix("MapTex")?.strip_suffix(".sbmaptex")?;
    let (level, rest) = match rest.as_bytes().first()? {
        d @ b'0'..=b'9' => ((d - b'0') as usize, &rest[1..]),
        _ => (0, rest),
    };
    let rest = rest.strip_prefix('_')?.as_bytes();
    let (&column, rest) = rest.split_first()?;
    if !column.is_ascii_uppercase() || rest.first() != Some(&b'-') {
        return None;
    }
    let digits = rest[1..].iter().take_while(|b| b.is_ascii_digit()).count();
    let tail = &rest[1 + digits..];
    if digits == 0 || !(tail.is_empty() || tail[0] == b'-' || tail[0] == b'_') {
        return None;
    }
    let row = std::str::from_utf8(&rest[1..1 + digits]).ok()?.parse().ok()?;
    Some(Position { level, column, row })
}

/// What [`make`] is asked to do.
#[derive(Debug, Clone)]
pub struct Options {
    /// The game's `romfs` folder.
    pub romfs: PathBuf,
    /// Where the mod is made: it gets `romfs/UI/MapTex/MainField/` with the map's tiles.
    pub mod_dir: PathBuf,
    /// Where the in-between images go (by default `<mod folder>-work`).
    pub work: Option<PathBuf>,
    /// `realesrgan-ncnn-vulkan`, with its `models` folder next to it.
    pub esrgan: PathBuf,
    /// 1: each tile at its own size; 2: the two closest levels at twice the size.
    pub scale: u32,
    /// Threads for decoding and encoding (by default half the CPU threads).
    pub jobs: Option<usize>,
    /// Only the tiles whose file names match, to try it on a few first.
    pub only: Option<glob::Pattern>,
}

/// The folders and settings every step reads.
struct Run {
    src: PathBuf,
    work: PathBuf,
    out: PathBuf,
    scale: u32,
    pads: [u32; 4],
    closest_size: OnceLock<Result<(u32, u32), String>>,
}

impl Run {
    fn read(&self, name: &str) -> Result<(Vec<u8>, tile::Tile)> {
        let path = self.src.join(name);
        tile::read(&fs::read(&path)?).with_context(|| path.display().to_string())
    }

    /// The base (no variant) tile at a grid position, or `None` off the grid.
    fn base_tile(&self, level: usize, column: usize, row: i64) -> Result<Option<Rgb>> {
        if column >= COLUMNS.len() || row < 0 {
            return Ok(None);
        }
        let digit = if level > 0 { level.to_string() } else { String::new() };
        let name = format!("MapTex{digit}_{}-{row}.sbmaptex", COLUMNS[column] as char);
        if !self.src.join(&name).is_file() {
            return Ok(None);
        }
        let (d, tile) = self.read(&name)?;
        tile::rgb(&d, &tile).map(Some)
    }

    fn level_dir(&self, level: usize, what: &str) -> PathBuf {
        self.work.join(format!("L{level}")).join(what)
    }

    /// The tile decoded with a border from its neighbours (the base tiles of the same level): `L<n>/in/<name>.png`.
    fn pad_one(&self, name: &str) -> Result<()> {
        let pos = position(name).context("not a map tile's name")?;
        let stem = name.trim_end_matches(".sbmaptex");
        let dst = self.level_dir(pos.level, "in").join(format!("{stem}.png"));
        if dst.exists() {
            return Ok(());
        }
        let (d, tile) = self.read(name)?;
        let img = tile::rgb(&d, &tile)?;
        let (p, w, h) = (self.pads[pos.level] as usize, img.width as usize, img.height as usize);
        let (bw, bh) = (w + 2 * p, h + 2 * p);
        // Edge copies where nothing is next to it.
        let mut big = vec![0u8; bw * bh * 3];
        for y in 0..bh {
            let sy = y.saturating_sub(p).min(h - 1);
            for x in 0..bw {
                let sx = x.saturating_sub(p).min(w - 1);
                big[(y * bw + x) * 3..][..3].copy_from_slice(&img.pixels[(sy * w + sx) * 3..][..3]);
            }
        }
        if let Some(ci) = COLUMNS.iter().position(|&c| c == pos.column) {
            for dy in [-1i64, 0, 1] {
                for dx in [-1i64, 0, 1] {
                    if dx == 0 && dy == 0 || ci as i64 + dx < 0 {
                        continue;
                    }
                    let Some(n) = self.base_tile(pos.level, (ci as i64 + dx) as usize, pos.row as i64 + dy)? else {
                        continue;
                    };
                    if (n.width, n.height) != (img.width, img.height) {
                        continue;
                    }
                    // Rows and columns: where they go in the padded image, and where they come from.
                    let span = |dir: i64, size: usize| match dir {
                        -1 => (0, size - p, p),
                        1 => (p + size, 0, p),
                        _ => (p, 0, size),
                    };
                    let ((ty, fy, ny), (tx, fx, nx)) = (span(dy, h), span(dx, w));
                    for row in 0..ny {
                        let to = ((ty + row) * bw + tx) * 3;
                        let from = ((fy + row) * w + fx) * 3;
                        big[to..to + nx * 3].copy_from_slice(&n.pixels[from..from + nx * 3]);
                    }
                }
            }
        }
        fs::create_dir_all(dst.parent().context("no parent")?)?;
        Rgb { width: bw as u32, height: bh as u32, pixels: big }.write_png(&dst)
    }

    /// One Real-ESRGAN run over the level's tiles not cleaned yet: `L<n>/in` -> `L<n>/x2`.
    fn esrgan(&self, exe: &Path, level: usize, progress: &(dyn Fn(String) + Sync)) -> Result<()> {
        let (src, dst) = (self.level_dir(level, "in"), self.level_dir(level, "x2"));
        if !src.is_dir() {
            return Ok(());
        }
        fs::create_dir_all(&dst)?;
        let todo: Vec<String> = fs::read_dir(&src)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|f| f.ends_with(".png") && !f.ends_with(".tmp.png") && !dst.join(f).exists())
            .collect();
        if todo.is_empty() {
            return Ok(());
        }
        // Real-ESRGAN takes a folder: one with only what is left.
        let left = self.level_dir(level, "todo");
        if left.exists() {
            fs::remove_dir_all(&left)?;
        }
        fs::create_dir_all(&left)?;
        for f in &todo {
            if std::fs::hard_link(src.join(f), left.join(f)).is_err() {
                fs::copy(src.join(f), left.join(f))?;
            }
        }
        let start = Instant::now();
        let status = Command::new(exe)
            .arg("-i")
            .arg(&left)
            .arg("-o")
            .arg(&dst)
            .args(["-n", MODEL, "-s", "2", "-f", "png"])
            .current_dir(exe.parent().context("Real-ESRGAN has no folder")?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| format!("Could not run {}", exe.display()))?;
        if !status.success() {
            bail!("Real-ESRGAN stopped with {status} on level {level}");
        }
        fs::remove_dir_all(&left)?;
        let missing = todo.iter().filter(|f| !dst.join(f).exists()).count();
        if missing > 0 {
            bail!(
                "Real-ESRGAN made {} of {} images for level {level}: is there a GPU it can use?",
                todo.len() - missing,
                todo.len()
            );
        }
        progress(format!(
            "  Real-ESRGAN, level {level}: {} tiles, {:.0} s",
            todo.len(),
            start.elapsed().as_secs_f64()
        ));
        Ok(())
    }

    /// Real-ESRGAN's image of a tile, and the border it has (at the tile's own size).
    fn x2(&self, level: usize, stem: &str, height: u32, width: u32) -> Result<(Rgb, u32)> {
        let p = self.pads[level];
        let x2 = Rgb::read_png(&self.level_dir(level, "x2").join(format!("{stem}.png")))?;
        if (x2.width, x2.height) != ((width + 2 * p) * 2, (height + 2 * p) * 2) {
            bail!("{stem}: Real-ESRGAN image is {}x{}", x2.width, x2.height);
        }
        Ok((x2, p))
    }

    /// The size of the closest level's tiles.
    fn closest_size(&self) -> Result<(u32, u32)> {
        self.closest_size
            .get_or_init(|| {
                let first = tile_names(&self.src)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .find(|f| position(f).is_some_and(|p| p.level == 0))
                    .ok_or("no tiles of the closest level")?;
                let (_, tile) = self.read(&first).map_err(|e| e.to_string())?;
                Ok((tile.height, tile.width))
            })
            .clone()
            .map_err(anyhow_ext::Error::msg)
    }

    /// The cleaned tile, compressed to BC1 again: `<mod>/romfs/UI/MapTex/MainField/<name>`.
    fn encode_one(&self, name: &str) -> Result<()> {
        let pos = position(name).context("not a map tile's name")?;
        let dst = self.out.join(name);
        if dst.exists() {
            return Ok(());
        }
        let (d, tile) = self.read(name)?;
        let (w, h) = (tile.width, tile.height);
        let stem = name.trim_end_matches(".sbmaptex");
        // ("_A-1_0": the same tile at every level)
        let rest = &stem["MapTex".len() + usize::from(pos.level > 0)..];
        let closest = format!("MapTex{rest}");
        let img = if self.scale == 2
            && pos.level <= 1
            && self.level_dir(0, "x2").join(format!("{closest}.png")).exists()
        {
            let (ch, cw) = if pos.level == 0 { (h, w) } else { self.closest_size()? };
            let (x2, p) = self.x2(0, &closest, ch, cw)?;
            let img = x2.crop(2 * p, 2 * p, x2.width - 4 * p, x2.height - 4 * p);
            if pos.level == 1 { lanczos::resize(&img, w * 2, h * 2) } else { img }
        } else {
            // (else: as with scale 1)
            let (x2, p) = self.x2(pos.level, stem, h, w)?;
            lanczos::resize(&x2, w + 2 * p, h + 2 * p).crop(p, p, w, h)
        };
        let out = tile::with(&d, &tile, &img).with_context(|| name.to_string())?;
        let temp = dst.with_extension("sbmaptex.tmp");
        fs::write(&temp, yaz0_store(&out))?;
        fs::rename(&temp, &dst)?;
        Ok(())
    }
}

/// The map tiles' file names in a folder, sorted.
fn tile_names(src: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = fs::read_dir(src)?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|f| position(f).is_some())
        .collect();
    names.sort();
    Ok(names)
}

/// Runs one step over the tiles on `jobs` threads; an error stops the run, and what is done stays done.
fn each(
    pool: &rayon::ThreadPool,
    files: &[String],
    what: &str,
    progress: &(dyn Fn(String) + Sync),
    step: impl Fn(&str) -> Result<()> + Sync,
) -> Result<()> {
    let start = Instant::now();
    let done = AtomicUsize::new(0);
    pool.install(|| {
        files.par_iter().try_for_each(|f| {
            step(f).with_context(|| format!("{what} {f}"))?;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 100 == 0 || n == files.len() {
                progress(format!("  {what}: {n}/{}, {:.0} s", files.len(), start.elapsed().as_secs_f64()));
            }
            Ok(())
        })
    })
}

/// Makes the mod. `progress` gets a line now and then; the whole map takes hours (on a GPU Real-ESRGAN can use).
/// Returns the work folder, which can be deleted afterwards.
pub fn make(options: &Options, progress: &(dyn Fn(String) + Sync)) -> Result<PathBuf> {
    let exe = std::path::absolute(&options.esrgan)?;
    if !exe.is_file() {
        bail!("{}: not found (realesrgan-ncnn-vulkan)", exe.display());
    }
    let models = exe.parent().context("Real-ESRGAN has no folder")?.join("models");
    if !models.join(format!("{MODEL}-x2.param")).is_file() {
        bail!("{}: no models/{MODEL}-x2.param next to it", exe.display());
    }
    if ![1, 2].contains(&options.scale) {
        bail!("The scale is 1 or 2");
    }
    let src = options.romfs.join(TILES);
    if !src.is_dir() {
        bail!("{}: no {TILES} (the game's romfs folder)", options.romfs.display());
    }
    let mod_dir = std::path::absolute(&options.mod_dir)?;
    let work = match &options.work {
        Some(work) => std::path::absolute(work)?,
        None => {
            let mut name = mod_dir.file_name().context("The mod folder has no name")?.to_os_string();
            name.push("-work");
            mod_dir.with_file_name(name)
        }
    };
    let out = mod_dir.join("romfs").join(TILES);
    fs::create_dir_all(&out)?;
    let stamp = mod_dir.join(STAMP);
    if stamp.exists() && fs::read_to_string(&stamp)? != options.scale.to_string() {
        bail!("{} was made with another scale: delete its romfs folder first", mod_dir.display());
    }
    fs::write(&stamp, options.scale.to_string())?;
    // The name and description the mod gets when it is installed.
    fs::write(
        mod_dir.join("rules.txt"),
        format!(
            "[Definition]\nname = MapHD\ndescription = A sharper world map made on this PC from your own game files \
             with Real-ESRGAN (scale {}). It holds your game's map textures: don't share it.\n",
            options.scale
        ),
    )?;
    let jobs = options
        .jobs
        .unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1))
        .max(1);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(jobs).build()?;
    let run = Run { src, work: work.clone(), out, scale: options.scale, pads: PADS, closest_size: OnceLock::new() };

    let mut files: Vec<String> = tile_names(&run.src)?
        .into_iter()
        .filter(|f| options.only.as_ref().is_none_or(|p| p.matches(f)))
        .collect();
    files.sort_by_key(|f| position(f));
    progress(format!("{} tiles; work folder {}", files.len(), work.display()));
    each(&pool, &files, "decoded", progress, |f| run.pad_one(f))?;
    for level in 0..4 {
        run.esrgan(&exe, level, progress)?;
    }
    each(&pool, &files, "encoded", progress, |f| run.encode_one(f))?;
    Ok(work)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
