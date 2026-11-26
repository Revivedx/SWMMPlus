use std::{
    io::{stdin, stdout, Write},
    option::Option,
    path::{Path, PathBuf},
};

use anyhow_ext::{Context, Result};
use smartstring::alias::String;
use swmm_core::{core, mods::LookupMod};
use uk_mod::{unpack::ModReader, Manifest, Meta};

use crate::ui::jobs;

xflags::xflags! {
    src "./src/cli.rs"

    cmd swmm {
        optional -d, --debug
        optional -p, --portable
        optional -D, --deploy
        cmd install {
            required path: PathBuf
            optional profile: String
        }
        cmd package {
            required path: PathBuf
            required output: PathBuf
            required meta: PathBuf
        }
        cmd uninstall {
            optional index: usize
            optional profile: String
        }
        cmd remerge {}
        cmd deploy {}
        cmd status {}
        /// A sharper world map made from your own game files: every map tile cleaned with Real-ESRGAN, as a mod.
        /// The whole map takes hours on a GPU. The mod holds your game's map textures, so don't share it.
        cmd maphd {
            /// Where the mod is made (it gets romfs/UI/MapTex/MainField/); install it like any mod afterwards.
            required folder: PathBuf
            /// realesrgan-ncnn-vulkan (https://github.com/xinntao/Real-ESRGAN/releases, the ncnn Vulkan
            /// download), with its models folder next to it: realesr-animevideov3-x2 is used.
            required --esrgan exe: PathBuf
            /// 1 (default): each tile cleaned at its own size. 2: the two closest zoom levels at twice the size,
            /// for windows above 1080p (about 11 GB more).
            optional --scale n: u32
            /// Where the in-between images go (default: <folder>-work, about 20 GB). A stopped run goes on where
            /// it was; delete the folder afterwards.
            optional --work dir: PathBuf
            /// Threads for decoding and encoding (default: half the CPU threads).
            optional --jobs n: usize
            /// The game's romfs folder (default: the one Savage Wind uses).
            optional --romfs dir: PathBuf
            /// Only the tiles whose file names match, for example MapTex*_D-3*, to try it on a few first.
            optional --only pattern: String
        }
    }
}
#[derive(Debug)]
pub struct Swmm {
    pub debug: bool,
    #[allow(dead_code)]
    pub portable: bool,
    pub deploy: bool,
    pub subcommand: SwmmCmd,
}

#[derive(Debug)]
pub enum SwmmCmd {
    Install(Install),
    Uninstall(Uninstall),
    Package(Package),
    Remerge(Remerge),
    Deploy(Deploy),
    Status(Status),
    Maphd(Maphd),
}

#[derive(Debug)]
pub struct Install {
    pub path:    PathBuf,
    pub profile: Option<String>,
}

#[derive(Debug)]
pub struct Package {
    pub path:   PathBuf,
    pub output: PathBuf,
    pub meta:   PathBuf,
}

#[derive(Debug)]
pub struct Uninstall {
    pub index:   Option<usize>,
    pub profile: Option<String>,
}

#[derive(Debug)]
pub struct Remerge;

#[derive(Debug)]
pub struct Deploy;

#[derive(Debug)]
pub struct Status;

#[derive(Debug)]
pub struct Maphd {
    pub folder: PathBuf,
    pub esrgan: PathBuf,
    pub scale: Option<u32>,
    pub work: Option<PathBuf>,
    pub jobs: Option<usize>,
    pub romfs: Option<PathBuf>,
    pub only: Option<String>,
}

impl Swmm {
    #[allow(dead_code)]
    pub fn from_env_or_exit() -> Self {
        Self::from_env_or_exit_()
    }

    #[allow(dead_code)]
    pub fn from_env() -> xflags::Result<Self> {
        Self::from_env_()
    }

    #[allow(dead_code)]
    pub fn from_vec(args: Vec<std::ffi::OsString>) -> xflags::Result<Self> {
        Self::from_vec_(args)
    }
}

macro_rules! input {
    () => {{
        stdout().flush()?;
        let mut _input = std::string::String::new();
        stdin().read_line(&mut _input).unwrap();
        _input
    }};
}

#[derive(Debug)]
pub struct Runner {
    core: core::Manager,
    cli:  Swmm,
}

impl Runner {
    pub fn new(cli: Swmm) -> Self {
        let core = core::Manager::init().unwrap();
        // The same automatic setup the window does on start: find Savage Wind and the player's game files.
        let saved = {
            let mut settings = core.settings_mut();
            match swmm_core::savage_wind::auto_configure(&mut settings) {
                Ok(done) if done.changed() => settings.save().map(|_| true),
                Ok(_) => Ok(false),
                Err(e) => {
                    eprintln!("Savage Wind isn't set up yet: {e:?}");
                    Ok(false)
                }
            }
        };
        match saved {
            Ok(true) => {
                if let Err(e) = core.reload() {
                    eprintln!("Failed to reload after setting up Savage Wind: {e:?}");
                }
            }
            Ok(false) => (),
            Err(e) => eprintln!("Failed to save the Savage Wind settings: {e:?}"),
        }
        Self { core, cli }
    }

    // Anything the window accepts: a packaged mod, a BNP, an archive or a folder of game files (import.rs).
    fn check_mod(&self, path: &Path) -> Result<Option<PathBuf>> {
        let (mod_, path) = match ModReader::open_peek(path, vec![]) {
            Ok(mod_) => (mod_, path.to_path_buf()),
            Err(e) => {
                let converted = swmm_core::import::convert_any(&self.core, path, None)
                    .with_context(|| format!("Could not open {} as a mod ({e})", path.display()))?;
                (
                    ModReader::open_peek(&converted, vec![]).context("Failed to open converted mod")?,
                    converted,
                )
            }
        };
        let notes = swmm_core::wiiu::take_notes();
        if !notes.is_empty() {
            println!("This is a Wii U mod. SWMM+ converted it on this PC for the Switch game. It may work, work in part or not work.");
        }
        for line in notes {
            println!("{line}");
        }
        if swmm_core::rebase::take_report().is_some() {
            println!("This mod was made for another version of the game. SWMM+ brought it to yours on this PC: what the mod changes goes in as it is, and what only your version has stays.");
        }
        println!("Installing {}...", mod_.meta.name);
        Ok(Some(path))
    }

    fn status(&self) -> Result<()> {
        let settings = self.core.settings();
        let game = swmm_core::savage_wind::current_game_files(&settings).ok();
        let mods = self
            .core
            .mod_manager()
            .mods()
            .enumerate()
            .map(|(index, m)| {
                serde_json::json!({
                    "index": index,
                    "name": m.meta.name.as_str(),
                    "version": m.meta.version.as_str(),
                    "author": m.meta.author.as_str(),
                    "enabled": m.enabled,
                    "options": m.enabled_options.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let status = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "executable": settings.savage_wind.executable,
            "game_version": game.as_ref().and_then(|g| g.version.clone()),
            "dlc_version": game.as_ref().and_then(|g| g.aoc_version.clone()),
            "overlay": swmm_core::savage_wind::overlay_dir(&settings),
            "pending_deploy": self.core.deploy_manager().pending(),
            "mods": mods,
        });
        println!("{}", serde_json::to_string_pretty(&status)?);
        Ok(())
    }

    fn deploy(&self) -> Result<()> {
        let deployer = self.core.deploy_manager();
        if deployer.pending() {
            println!("Deploying changes...");
            deployer.deploy()?;
            println!("Deployment complete");
        } else {
            println!("No changes pending deployment");
        };
        Ok(())
    }

    fn maphd(&self, args: &Maphd) -> Result<()> {
        let romfs = match &args.romfs {
            Some(romfs) => romfs.clone(),
            None => swmm_core::savage_wind::current_game_files(&self.core.settings())?.romfs,
        };
        let only = args
            .only
            .as_deref()
            .map(glob::Pattern::new)
            .transpose()
            .context("--only isn't a file name pattern")?;
        let options = swmm_core::maphd::Options {
            romfs,
            mod_dir: args.folder.clone(),
            work: args.work.clone(),
            esrgan: args.esrgan.clone(),
            scale: args.scale.unwrap_or(1),
            jobs: args.jobs,
            only,
        };
        let work = swmm_core::maphd::make(&options, &|line| println!("{line}"))?;
        println!(
            "Made {}: install it like any mod (drop the folder on the window, or swmm install). The work folder \
             ({}) can be deleted.",
            args.folder.display(),
            work.display()
        );
        println!("The mod holds your game's map textures, changed: don't share it.");
        Ok(())
    }

    pub fn run(self) -> Result<()> {
        if self.cli.debug {
            env_logger::init();
            log::set_max_level(log::LevelFilter::Debug);
        }
        match &self.cli.subcommand {
            SwmmCmd::Install(Install { path, profile }) => {
                if let Some(path) = self.check_mod(path)? {
                    let mods = self.core.mod_manager();
                    let mut mod_ = mods.add(&path, profile.as_ref())?;
                    mods.set_enabled(mod_.as_map_id(), true, profile.as_ref())?;
                    // No window to ask in: a mod with options gets the ones its author marked as default.
                    mod_.enable_default_options();
                    // A group that needs a choice and names no default gets its first option.
                    for group in &mod_.meta.options {
                        if let uk_mod::OptionGroup::Exclusive(group) = group {
                            let chosen = group.options.iter().any(|o| mod_.enabled_options.contains(o));
                            if group.required && !chosen {
                                mod_.enabled_options.extend(group.options.first().cloned());
                            }
                        }
                    }
                    if !mod_.enabled_options.is_empty() {
                        for option in &mod_.enabled_options {
                            println!("Option: {}", option.name);
                        }
                        mods.set_enabled_options(mod_.as_map_id(), mod_.enabled_options.clone())?;
                    }
                    mods.save()?;
                    println!("Applying mod to load order...");
                    let deployer = self.core.deploy_manager();
                    deployer.apply(Some(mod_.manifest()?.as_ref().clone()))?;
                    if self.cli.deploy {
                        self.deploy()?;
                    }
                    println!("Done!");
                }
            }
            SwmmCmd::Package(pkg) => {
                println!("Packaging mod...");
                let builder = jobs::PackageJob {
                    source: pkg.path.clone(),
                    dest:   pkg.output.clone(),
                    meta:   Meta::parse(&pkg.meta)?,
                };
                jobs::package_mod(&self.core, builder)?;
                println!("Done!");
            }
            SwmmCmd::Remerge(_) => {
                println!("Remerging...");
                jobs::apply_changes(&self.core, vec![], None)?;
                println!("Done!");
            }
            SwmmCmd::Uninstall(Uninstall { index, profile }) => {
                let mut manifests = Manifest::default();
                let mod_manager = self.core.mod_manager();
                let mods = mod_manager.mods().collect::<Vec<_>>();

                if let Some(index_value) = index {
                    let mod_ = mods
                        .get(*index_value)
                        .with_context(|| format!("Mod {} does not exist", index_value))?;
                    println!("Removing mod {}...", &mod_.meta.name);
                    mod_manager.del(mod_, profile.as_ref())?;
                    mod_manager.save()?;
                    manifests.extend(mod_.manifest()?.as_ref());
                } else {
                    println!("Installed mods:");
                    for (i, mod_) in mods.iter().enumerate() {
                        println!(
                            "{}. {} (v{}) by {}",
                            i + 1,
                            &mod_.meta.name,
                            &mod_.meta.version,
                            &mod_.meta.author
                        );
                    }
                    print!("Enter mod(s) to uninstall, separated by commas: ");
                    let mut manifests = Manifest::default();
                    for id in input!().replace(' ', "").split(',') {
                        let mod_ = mods
                            .get(id.trim().parse::<usize>().context("Invalid mod number")? - 1)
                            .with_context(|| format!("Mod {} does not exist", id))?;
                        println!("Removing mod {}...", &mod_.meta.name);
                        mod_manager.del(mod_, profile.as_ref())?;
                        mod_manager.save()?;
                        manifests.extend(mod_.manifest()?.as_ref());
                    }
                }

                println!("Applying changes to merge...");
                self.core.deploy_manager().apply(Some(manifests))?;
                if self.cli.deploy {
                    self.deploy()?;
                }
                println!("Done!");
            }
            SwmmCmd::Deploy(_) => self.deploy()?,
            SwmmCmd::Status(_) => self.status()?,
            SwmmCmd::Maphd(args) => self.maphd(args)?,
        };
        Ok(())
    }
}
