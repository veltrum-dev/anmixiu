# Scene benchmark baseline

Recorded 2026-08-26 on an Apple M1 Pro (arm64, 16 GiB), Rust 1.98.0, release profile.
Criterion used 100 ms warm-up, 200 ms measurement, and 10 samples.

| Case | Scale | Median estimate |
| --- | ---: | ---: |
| Scene command build | 100 quads | 323.01 ns |
| Scene command build | 10,000 quads | 27.144 us |
| Scene cache hit | steady state | 39.395 ns |

The cache has a caller-selected non-zero hard capacity; the contract tests verify LRU
eviction and that steady-state entry count does not grow past it. Allocation count,
allocated bytes, and process RSS were not instrumented in this baseline: a global
allocation counter requires unsafe allocator hooks, which this crate's safety contract
forbids. Those metrics remain an external Instruments/profiler verification item.
