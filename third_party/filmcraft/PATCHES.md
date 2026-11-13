# VideoLab's patch to FilmCraft

FilmCraft `db60ee45c2f5a5a125f26e4f6fc5de0b74fb811b`: the `render` and `gpu` crates, copied
unchanged except for the patch below. VideoLab's `Cargo.toml` routes `filmcraft-render` and
`filmcraft-gpu` here with `[patch]`; every other FilmCraft crate stays the git dependency at the
same rev. The small workspace here (`Cargo.toml`) points those crates at the same rev, patches
the two crates to these copies for FilmCraft's own git crates too, and carries FilmCraft's build
profiles so its tests behave as upstream.

To drop the patch: remove the `[patch]` section from VideoLab's `Cargo.toml` (and the
`exclude = ["third_party"]` line can go with this folder).

## Lumetri's looks, curves and wheels on the GPU

**Problem.** `FxOp::eval` gave Lumetri a GPU op only without a Creative Look, Look LUT, Curves or
Color Wheels (`gpu_capable`). Any of them sent the clip's whole effect chain to the CPU every
frame. VideoLab's filters are Lumetri presets (`lumetri.applyPreset`), so every filtered clip
did: on a phone, two Lumetri effects on a 4K clip cost about 25 ms per preview frame
(35.7 ms per frame with them, 10.5 without; same clip, Adreno 840).

**Change.** Those sections are one per-pixel function of the colour. For SDR it is baked into a
65³ table over the sRGB-encoded unit cube (`gpufx::ColorLut`, cached per setting by
`gpufx::lumetri_advanced_lut`) and applied right after the basic grade:

- `render/src/effects.rs`: `lumetri_advanced` split into `lumetri_advanced_map`, the per-pixel
  function (no behaviour change for the CPU path that still uses it).
- `render/src/gpufx.rs`: `ColorLut` (bake, trilinear `sample`), `FxOp::Lumetri::advanced`,
  `gpu_capable` no longer blocked by a look, curves or wheels that baked; `FxOp::apply` applies
  the table after the basic grade (the CPU reference, so CPU and GPU renders agree).
- `gpu/src/fx.rs`, `gpu/src/fx.wgsl`: `OP_COLOR_LUT`, the same trilinear sampling in WGSL; the
  table is the pass's aux texture (`n²` × `n`), uploaded once per table and kept while drawn.

Still on the CPU: Sharpen (spatial), Input LUT, HSL Secondary, and HDR working spaces.

**Accuracy.** Within one 8-bit step of the exact function everywhere
(`baked_lumetri_grade_matches_the_exact_function`): most looks, curves and wheels within 0.06 of
a step; the worst, 0.66 to 0.73, along the clamp of a look's S-curve or a wheel. GPU and CPU
agree to 1e-4 (`fx_tests::gpu_effects_match_cpu_exactly`, new Lumetri cases).
`lumetri_unsupported_sections_fall_back` now expects looks, curves and wheels on the GPU.

**Effect.** 15.6 ms per frame instead of 35.7 on the same clip; the reel plays at the sequence's
30 fps.
