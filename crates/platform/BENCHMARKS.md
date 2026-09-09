# Taffy layout benchmark baseline

Recorded 2026-08-26 on an Apple M1 Pro (arm64, 16 GiB), Rust 1.98.0, release profile.
Criterion used 100 ms warm-up, 200 ms measurement, and 10 samples.

| Case | Scale | Median estimate |
| --- | ---: | ---: |
| Uncached Flexbox layout | 100 nodes | 27.306 us |
| Uncached Flexbox layout | 5,000 nodes | 1.5098 ms |
| Exact-key cache hit | 1,000-node tree | 9.7589 ns |

`LayoutEngine` deliberately retains at most one `Arc<LayoutTree>`; its contract tests
verify replacement on structure/style/measurement/resize/scale invalidation and a hard
steady-state cache size of one. Allocation count, allocated bytes, and process RSS were
not instrumented in this baseline: a global allocation counter requires unsafe allocator
hooks, which this crate's safety contract forbids. Those metrics remain an external
Instruments/profiler verification item.
