# Metal performance record

Measured 2026-08-26 on a 16 GiB Apple M1 Pro (`MacBookPro18,3`), macOS 26.5.1,
with `cargo bench -p anmixiu-render-metal`. The benchmark renders a 256×256 RGBA8
texture and includes command encoding, one submit, GPU completion wait, and CPU
readback; it therefore does not under-report asynchronous GPU work as submit-only time.

| Scale | Draw commands | Time | Allocations / frame | Allocated bytes / frame |
| --- | ---: | ---: | ---: | ---: |
| Normal | 1 | 449.29–496.45 µs | 1 | 262,144 B |
| Stress | 1,000 | 945.25–982.31 µs | 1 | 262,144 B |

The single allocation is the explicit RGBA readback buffer. Across 100 iterations,
allocated and deallocated bytes were both 26,214,400 B, with zero reallocations and
zero measured net CPU heap growth. This no-glyph scene retained 0 atlas bytes; glyph
textures are independently hard-limited by `RendererConfig::atlas_texture_capacity`
and exposed through `RenderStats::cached_atlas_bytes`. Driver-private and transient GPU
memory are not observable by this allocator probe.

Reproduce timing with
`cargo bench -p anmixiu-render-metal --bench metal_submit -- --noplot` and allocation
data with `cargo bench -p anmixiu-render-metal --bench allocation`.
