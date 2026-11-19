# SWMM+ plan

SWMM+ is the mod manager for [Savage Wind](https://github.com/Revivedx/SavageWindRecomp), kept by Savage Wind's
maintainer. It is a fork of [SWMM](https://github.com/ustruct/SWMM) by ustruct, which is built on
[U-King Mod Manager](https://github.com/NiceneNerd/ukmm) by Caleb Smith and Ginger Chody. Like both, it is free
software under the GNU General Public License v3.0, and it stays a separate program: Savage Wind (MIT) offers to
download it on request and reads what it deploys, and the two only talk through files and the command line.

Mods are other people's work. SWMM+ doesn't host, check or support them, and isn't responsible for what they contain
or do; bugs caused by mods are closed.

## Phase 1: the Savage Wind 1.9.0 manager (target: Savage Wind Alpha 7)

1. **Nothing inside the game.** Remove the loader DLL that sits next to `savagewind.exe`, the persistent
   `SAVAGEWIND_ROMFS_OVERLAY` environment variable and the Steam shortcut edits. Savage Wind reads SWMM+'s deploy
   folder itself, as one layer in its Mods tab, with its own switch and place in the load order.
2. **A command line Savage Wind can drive**: status, install, uninstall, enable, order, deploy and open, with
   machine-readable output. BNPs install from it too, with their default options.
3. **Right for game version 1.9.0.** The ResourceSizeTable starts from the player's own 1.9.0 table, never from a
   bundled 1.6.0 one (the 1.6.0 table made the game crash at boot). What counts as "stock" comes from the player's
   own game files. A pack a mod touches changes only the files the mod changes; everything else keeps the game's own
   bytes. The game's `Layout/Common` stays unless a mod is made for 1.9.0. Round-trip tests over the 1.9.0 files.
4. **GameBanana browser**, off until the player opens it and accepts a notice that GameBanana is a third-party site
   and SWMM+ only searches it. Every install shows that the mod is other people's work, may contain content that
   belongs to others and may break the game or saves. Downloads go straight from GameBanana; nothing is mirrored.
5. **Releases**: Windows `.zip` and Linux `.tar.xz` with SHA-256 sums, built by CI from this repository.

## Phase 2: Wii U mods

1. Data files that only differ in byte order (BYML, AAMP, SARC, MSBT, event flows): converted to the Switch's.
2. Textures (`.bflim`, textures in `.bfres`): GX2 tiling to the Switch's, into the game's `.bntx`.
3. Fonts (`.bffnt`): byte order and glyph sheets.

Wii U models and Cemu graphic packs (shaders, code patches) are not convertible.

## Savage Wind side

Configuration > Mods gets an SWMM+ section: Install (a pinned version, SHA-256 checked, like DLSS), Open, Update and
Remove. What SWMM+ deploys shows up as a layer in the Mods tab. Tested with Savage Wind testers before it reaches
`test`.
