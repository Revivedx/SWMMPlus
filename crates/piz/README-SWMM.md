# piz, as SWMM+ uses it

This is [piz](https://github.com/mrkline/piz-rs) 0.5.1 (zlib licence, `LICENSE.md`), the zip reader UKMM's `uk-mod`
opens packaged mods with, with one change. The workspace's `Cargo.toml` puts it in place of the crates.io release
(`[patch.crates-io]`).

The change, in `parse_extra_field` (`src/spec.rs`): a Zip64 field with bytes left after the sizes and offset it
holds is accepted, and those bytes skipped. piz took them for a disk number and refused the archive. The zip crate
(2.x, which `uk-mod` packages mods with) writes the header offset into each local header's Zip64 field for entries
that start past 4 GiB, so every packaged mod bigger than that failed to open: "Invalid Zip archive: Extra data field
contains disk number". Multi-disk archives are still refused where piz checks for them.

`tests/local_zip64_offset.rs` is a small archive laid out that way. Drop this copy if piz or the zip crate fixes it.
