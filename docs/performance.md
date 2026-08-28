# Performance targets and measurement

The engineering reference budget is 8.33 ms for a 120 Hz display and 16.67 ms from ordinary input
to its next visible frame. These are environment-dependent targets, not API guarantees.

The release Counter baseline with two Tokio workers measured about 31 MB physical footprint at
idle on the reference M1 Pro host. Repeated live resize raised the footprint because Retina
`CAMetalLayer` IOSurfaces grew from roughly 10 MB to 33 MB; Rust `MALLOC_SMALL` remained near
11–13 MB. The observed increase is a graphics high-water mark, not evidence of heap growth. Active resize
may consume a substantial fraction of one CPU core because each coalesced size change performs
layout, scene construction, drawable resize, and a presented frame; idle CPU must return to zero.

Criterion benches cover Signal notification/dirty deduplication, Taffy layout, scene cache reuse,
Metal submission/offscreen drawing, and CoreText shaping/atlas work. Run all release benches with:

```sh
cargo bench --workspace
```

Every benchmark should record its normal and stress sizes. Allocation counts and allocated bytes
must be captured with an allocator profiler when comparing an algorithm change; Criterion timing
alone is not allocation evidence. Cache stats and hard capacities are asserted in tests so a warm
steady-state workload cannot grow bookkeeping without bound.

## MVP release baseline

Recorded on 2026-08-26 on an Apple M1 Pro (arm64, 16 GiB), macOS 26.5.1, Rust 1.98.0,
using the final workspace and `cargo bench --workspace`:

| Path | Normal | Stress |
| --- | ---: | ---: |
| Shared UI string clone | long label: 9.66–9.67 ns | 1,000 clones: 0 allocated bytes |
| Shared short formatting | `Count 42`: 5.75–5.77 ns | 1,000 static labels: 0 allocated bytes |
| macOS frame projection | 100 cached buttons: 59.4–60.3 µs | hover Scene rebuild: 82.9–91.9 µs |
| Frame dirty dedup + drain | 32 components: 2.55–2.66 µs | 4,096: 572.8–574.2 µs |
| Signal notify + dirty take | 1 owner: 197.5–203.5 ns | 1,000: 84.6–86.9 µs |
| Duplicate dirty insertion | 10 writes: 207.8–219.9 ns | 100,000: 1.584–1.586 ms |
| Taffy uncached Flexbox | 100 nodes: 25.01–25.17 µs | 5,000: 1.381–1.391 ms |
| Taffy exact-key cache hit | 1,000 nodes: 9.78–10.23 ns | — |
| Scene command build | 100: 337.8–343.8 ns | 10,000: 27.37–27.51 µs |
| Scene cache hit | 39.8–41.8 ns | — |
| CoreText cached shaping with system UI font | normal: 29.24–29.54 µs | mixed Latin/CJK: 71.69–72.00 µs |
| Metal submit + completion + readback | 1 draw: 388.3–414.4 µs | 1,000: 825.7–854.7 µs |

The Metal benchmark includes a 256×256 CPU readback and therefore is stricter than an onscreen
asynchronous present. Its explicit readback allocation is 262,144 B/frame; 100 iterations measured
equal allocated/deallocated bytes. CoreText's warm 1,000-iteration probes likewise measured equal
allocated/deallocated bytes and a fixed 1 MiB atlas. Detailed allocation and before/after CoreText
optimization evidence live in the renderer/text crate performance records. Scene and layout use
safe-code-only crates, so allocator-hook counts remain unverified there; their bounded entry counts
and steady-state reuse are contract-tested instead.
