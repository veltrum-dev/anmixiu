# CoreText performance record

Measured 2026-08-27 on a 16 GiB Apple M1 Pro (`MacBookPro18,3`), macOS 26.5.1,
with `cargo bench -p anmixiu-text-coretext`. Times are Criterion 95% intervals for
a warm atlas and the current macOS system UI font; allocation figures use `stats_alloc` around
the operation itself.

| Scale | Text | Time | Allocations / call | Allocated bytes / call | Steady resident atlas |
| --- | --- | ---: | ---: | ---: | ---: |
| Normal | `Counter 42 / Ready / 你好` | 29.24–29.54 µs | 55 | 9,037 B | 1,048,576 B |
| Stress | 78 mixed Latin/CJK characters | 71.69–72.00 µs | 164 | 35,884 B | 1,048,576 B |

The 1,000-iteration allocation probe reported equal allocated/deallocated bytes
(normal: 9,037,000 B; stress: 35,884,000 B), so the warm path had zero measured
net CPU heap growth. The atlas entry count is separately hard-limited by
`AtlasConfig::max_entries`.

Replacing legacy hard-coded Helvetica with `CTFontCreateUIFontForLanguage` makes the renderer
follow the current macOS UI font and its CJK fallback. Looking that font up on every shape initially
cost 56.74–57.43 µs normal and 111.15–140.70 µs stress. A one-entry base-font cache, keyed by exact
point-size bits and replaced on size change, reduced that to the table above without unbounded
growth. The remaining increase over the Helvetica baseline is an intentional native typography
quality tradeoff and remains below 1% of the 8.33 ms reference frame budget per shaped line.

Moving the atlas-hit check ahead of CoreGraphics rasterization improved normal
cached shaping from 132.56–134.00 µs to 22.61–22.86 µs (83.1%) and stress from
416.26–419.99 µs to 46.88–47.03 µs (87.8%).

The position-aware path includes the final device-space x position in a four-phase glyph cache key.
The quad starts at `floor(x * scale)` while the CoreText mask retains the quantized fractional
phase, preserving spacing without linearly resampling the mask at 1x. Rasterization now uses an
RGB32 CoreGraphics context followed by A8 extraction instead of requesting font smoothing from a
grayscale-only context. The additional phase variants remain
bounded by `AtlasConfig::max_entries`; warm allocation and deallocation totals remain equal.

Vertical placement now rounds one shared physical baseline and then applies each raster mask's
integer top bearing. The previous per-glyph top rounding could move mixed Latin/CJK glyphs by one
physical pixel at 1x. The baseline contract test reproduces the old mismatch and covers the fixed
placement without adding a steady-state cache or allocation.

Glyph UVs now include a verified transparent two-pixel safety border instead of cropping to the
nominal CoreText bounding rectangle. CoreGraphics font smoothing and subpixel positioning remain
enabled during rasterization. The safety-border test found nonzero edge alpha in the previous
cropped representation; the current outer UV perimeter is required to remain transparent. Atlas
resident memory remains fixed at 1 MiB and warm allocation/deallocation totals remain equal.

Reproduce timing with `cargo bench -p anmixiu-text-coretext --bench text -- --noplot`
and allocation data with `cargo bench -p anmixiu-text-coretext --bench allocation`.
