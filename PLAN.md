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

A Wii U mod (a BNP, or `content` and `aoc` folders) installs like any other. Its files are converted when it is
installed, and what can't be converted is left out, so the game keeps its own, and listed for the player.

Done:

1. Data files that only differ in byte order (BYML, AAMP, SARC, texts): the merger reads either.
2. Layout images (`.bflim`): out of the Wii U's tiling, into the Switch's, into the layout archive's `.bntx`.
3. Layouts (`.bflyt`) and layout animations (`.bflan`): version 5.2 big endian to version 8.0 little endian.
4. Fonts (`.bffnt`): version 3 to 4.1, glyph sheets into the texture archive inside the font.
5. Texture files (a Wii U model file with nothing but textures: icons `.sbitemico`, and the same shape elsewhere):
   each texture and its smaller levels out of the Wii U's tiling, into a texture archive inside the Switch's model
   file. The Switch file and the levels' places match the game's own files byte for byte.
6. A model's textures (`.Tex1.sbfres`) become the Switch's `.Tex.sbfres`. The Wii U file holds each texture at
   full size only (the smaller levels are in `.Tex2.sbfres`, which mods don't bring and the Switch doesn't have).
   Textures the mod leaves alone are taken from the game's own Switch file, levels and all; the ones it changes
   get new smaller levels, made here (BC1, BC3, BC4, BC5 and plain formats).
7. The player is told a mod is a Wii U one, that it was converted on their PC and that it may not work, with what
   was converted and what was left out (a notice in the window, lines in the command line).

To do:

1. Layouts with a parts pane that overrides a text box or a pane's placement, projected textures, animated user
   data: refused for now, the game's own layout stays.
2. A whole Wii U layout archive replaces the Switch's layouts with the Wii U's, changed by the mod or not: tell which
   ones the mod changes and convert only those.
3. Array, cube and 16-bit packed textures are refused. Textures inside a file that also holds models aren't
   read.
4. Sounds (`.bfstm`, `.bars`).

Wii U models, physics and Cemu graphic packs (shaders, code patches) are not convertible.

## Phase 3: mods made for other versions of the game

Mods are made on version 1.6.0. Savage Wind runs 1.9.0, which differs from it in some 90 files and has 19 new ones
(new actors and saved flags, two more languages, layouts, fonts).

Done:

1. BNPs and packaged mods carry their changes, not whole files: they go on 1.9.0 as they are.
2. A mod's unchanged copies of 1.6.0 files are left out by the packager, which knows them by their hashes.
3. A mod that comes as whole game files brings 1.6.0's table of actors and its saved flags. Installed as it was,
   it took the ten actors 1.9.0 added out of the game, gave 2375 actors their old memory sizes and took out 14
   saved flags. Before packaging, those files now get what only the player's game has, and the game's larger
   memory sizes (`rebase.rs`). The same mod installed from its whole files and from its BNP now gives the same
   game files, but for one 1.6.0 flag the newer game dropped.

4. Texts: such a mod also brings each language's whole archive of 1.6.0's texts. 1.9.0 reworded thirty to eighty
   of them in each language and replaced the credits' thousand, and all of those went back in with the mod. A list
   of numbers that stand for those 1.6.0 texts (`stale_texts.bin`, 13644 of them over 14 languages, no text in it)
   tells them from the mod's own work: a text on the list gives way to the game's (`texts.rs`). The mod's own
   texts go in as they are.

5. The player is told when a mod was brought to their version, in the window and on the command line. BNPs and
   packaged mods get no notice: nothing is done to them.

To do:

1. A mod that really takes an actor or a saved flag out of the game no longer does.
2. The list of texts covers mods made on Switch 1.6.0. A Wii U mod's whole text archives (1.5.0) aren't covered.

## Savage Wind side

Configuration > Mods gets an SWMM+ section: Install (a pinned version, SHA-256 checked, like DLSS), Open, Update and
Remove. What SWMM+ deploys shows up as a layer in the Mods tab. Tested with Savage Wind testers before it reaches
`test`.
