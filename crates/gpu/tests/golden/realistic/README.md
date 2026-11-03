# Realistic view golden frames

Golden GPU frames of `examples/realistic-showcase.json` (docs/material-graph.md §5.9): the actual
viewport on a native GPU, 1600 × 1000 with 2 × 2 supersampling, two camera poses, Standard and
Realistic looks.

- Track X writes the showcase; track I renders the frames here and adds the acceptance test
  (per-part clipped/crushed fractions, face gradients, medians, WebGL2 vs native, Standard
  bit-identical).
- Frames are regenerated only by an explicit script, never by a test run.
- **The founder reviews these frames before anything goes upstream.**
