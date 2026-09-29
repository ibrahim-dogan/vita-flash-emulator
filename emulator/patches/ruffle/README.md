# Ruffle core, patched for the Vita

A copy of Ruffle's `ruffle_core` and `ruffle_common` crates at commit
[`563854a8911e`](https://github.com/ruffle-rs/ruffle/tree/563854a8911eda672b1836fb374063162879035d),
the same commit every other Ruffle crate is pinned to in `../../Cargo.toml`.
`[patch]` in that file swaps these in for the git versions. Only the Cargo
manifests (git dependencies instead of workspace paths) and the files below
differ from upstream; every change is marked with a `RuffleVita:` comment.

Ruffle is licensed under MIT or Apache-2.0; see `LICENSE.md`.

## Changes

**Faster AVM1 (ActionScript 1/2).** Found profiling A Koopa's Revenge 2,
which went from about 3 to about 12 FPS on the Vita.

- `core/src/display_object/container.rs`: containers index their children by
  name once they have at least 16 and are searched repeatedly. AVM1 looks up
  a child by name on every property access that misses the object itself
  (`_x`, prototype methods, ...), and this was a linear scan.
- `core/src/display_object.rs`, `core/src/display_object/movie_clip.rs`,
  `core/src/avm1/object_reference.rs`, `core/src/avm1.rs`: each movie clip
  keeps the last `MovieClipReference` made to it and reuses it until any
  rename, reparent or level change, instead of building the clip's path
  string every time it is used as a value.
- `core/src/avm1/property_map.rs`, `core/common/src/avm_string/`: property
  names hash with a single multiply per character, and `AvmString`s memoize
  that hash, which was recomputed for every object on the prototype chain.

**Less memory for bitmaps.**

- `core/src/bitmap/bitmap_data.rs`: a `BitmapData`'s CPU pixel buffer is only
  allocated when ActionScript reads or writes pixels. Until then the bitmap is
  a single fill colour, and a transparent one starts as an empty GPU texture.
  Games that draw huge bitmaps on the GPU (Happy Wheels creates about 180 MB
  of them) no longer keep a second copy in RAM.

## Updating

Copy `core/` and `core/common/` from the new Ruffle commit, re-apply the
changes above (search upstream for the functions named here), point the
`rev` in `../../Cargo.toml`, `../../ruffle_render_glow/Cargo.toml` and these
two manifests at the new commit, and restore the git dependencies in the
manifests.
