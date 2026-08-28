# macOS frame projection performance

Measured 2026-08-27 on an Apple M1 Pro with Rust 1.98.0 in the release profile.

| Operation | Scale | Time |
| --- | ---: | ---: |
| Exact-key frame projection/cache reuse | 100 buttons | 59.4–60.3 µs |
| Hover target change + Scene rebuild | 100 buttons | 82.9–91.9 µs |

Hover changes only the bounded Scene paint key. The contract test verifies that the existing Taffy
layout `Arc` is reused and layout miss count does not increase. Normal non-hover nodes borrow their
projected `Style`; only the hovered node clones and applies its paint-only `StyleRefinement`.

The cached projection baseline includes intrinsic button sizing, centered label placement, cursor
metadata, and focus-ring paint commands. At roughly 0.060 ms for 100 buttons it remains far below
the 8.33 ms reference frame budget; the added correctness and control feedback are retained.
Cross-display frame requests now run on the `NSView` display link, and drawables whose physical
dimensions do not match the current backing scale are skipped rather than submitted.
During live resize, the latest viewport is coalesced and `CAMetalLayer.drawableSize` is applied at
the next draw boundary. Presentation is coordinated with the AppKit transaction and waits only for
GPU scheduling; this avoids stretching a stale drawable without blocking for GPU completion.
The initial surface is attempted synchronously after attachment, and an unavailable first drawable
gets one bounded follow-up display turn instead of waiting for a later resize event or entering a
busy retry loop.

Reproduce with:

```sh
cargo bench -p anmixiu-platform-macos --bench frame_builder -- --noplot
```
