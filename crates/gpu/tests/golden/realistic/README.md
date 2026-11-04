# Realistic view golden frames

Golden GPU frames of `examples/realistic-showcase.json` (docs/material-graph.md §5.9): the actual
viewport on a native GPU, 1600 × 1000, both looks, every saved view of the showcase. The
modelling view uses 2 × 2 supersampling; the Render preview draws as the app does, without it.

- Frames are regenerated only by an explicit run, never by a test run. The PNGs are about 3 MB a
  set and depend on the GPU, so they stay out of the repository. This folder keeps the metrics
  they were measured with (`showcase-metrics.jsonl`) as the record to compare against.
- **The founder reviews the frames before anything goes upstream.**

## Regenerate

Save the showcase, then draw it:

```sh
cargo run --release -p solidcraft-cli -- run examples/realistic-showcase.json --save showcase.solidcraft
GOLDEN_DESIGN=showcase.solidcraft GOLDEN_OUT=<dir> \
    cargo test --release -p solidcraft-ui-egui --lib golden_frames -- --ignored --nocapture
```

`crates/ui-egui/src/viewport_golden_tests.rs` documents the options:

- `GOLDEN_STUDIO` and `GOLDEN_INTENSITY` select another studio for comparisons.
- `GOLDEN_CHECK=1` makes the run the acceptance test. It fails on any §5.9 failure in the
  Realistic frames: clipped or crushed above 1 % per part, a face median outside 20–235, a
  planar metal face spanning under 12, or a Sample Plate top or side median outside 40–220.

Path-tracer frames of the same views (graph looks drawn by the path back end, with the job's
approximations) come from `crates/gpu/src/path/golden_tests.rs`:

```sh
GOLDEN_DESIGN=showcase.solidcraft GOLDEN_OUT=<dir> GOLDEN_EV=-6.44 GOLDEN_AMBIENT=30 \
    cargo test --release -p solidcraft-gpu --lib path_golden_frames -- --ignored --nocapture
```

The lighting review set (the Sample Plate and simple shapes, per material and pose) comes from
`crates/ui-egui/src/gpu_lighting_review.rs`.

## Standard and Realistic parity

`cargo xtask render-parity --base REV [--realistic-base REV]` draws a fixed set of small viewport
frames in this tree and in a worktree of `REV` on the same GPU. The set is
`crates/gpu/src/view/parity_frames*.rs`: PBR, scalar, line, occlusion, sky, section, analysis,
highlight, see-through and product-preview paths, and in the Realistic set every studio preset,
the room, and graph looks with coats, coat colour, anisotropy and patterns.

- Standard frames must equal the base's byte for byte.
- Realistic frames must be within `--tolerance` (default 1) of the realistic base.

Recorded on an Apple M5 Max (Metal) at `45427568d`:

| Check | Result |
|---|---|
| Standard against upstream `21c5be2b2` (before the package and the renderer branch) | 312 of 312 frames byte-identical |
| Realistic against `6790d434f` (the package before C2's anisotropic studio lobe) | 878 of 910 within 1 (877 identical). The 32 others are every brushed nickel silver frame, the anisotropic graph C2 changed on purpose (steps up to 14) |

## Acceptance against §5.9 (recorded at `db8e4d420`, default studio)

The bar is not met yet. In the modelling view's Realistic frames:

- Clipping and crushing pass. Every part is at or under 1 % in all three views, except the
  silicone dome's 1.9 % clipped from the low back view.
- Face medians fail:
  - the brushed lid's top reads 246–251;
  - the Sample Plate's top reads 245 in the isometric view, where the bar for the Sample Plate is
    40–220 (its side, 106, passes);
  - the black epoxy box's faces sit at 7–18.
- Gradients fail on planar metal faces under 12/255: the lid's faces, the plate's narrow sides and
  boss ring, and the cylinder's top from the low back view.

In the Render preview's Realistic frames, graph looks draw only as their base response: there is
no weave, label or transmission. From the low back view the epoxy box is 44 % crushed, the
carbon plate 7 % crushed and the brushed lid 1.2 % clipped.

`showcase-metrics.jsonl` holds every number: per part, and per face of at least 400 pixels.
