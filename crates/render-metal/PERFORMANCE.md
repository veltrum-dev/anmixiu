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

## Backdrop compositor measurement

Measured 2026-09-03 on the same Apple M1 Pro. These offscreen benchmarks include command encoding,
GPU completion, and CPU readback, so they are deliberately stricter than asynchronous onscreen
presentation.

| Workload | Time |
| --- | ---: |
| 256×256 direct, one quad | 411.29–421.70 µs |
| 256×256 direct, 1,000 quads | 865.90–894.75 µs |
| 256×256 backdrop blur, sigma 16 | 738.97–760.19 µs |
| 256×256 subtree filter blur, sigma 10 | 815.54–902.44 µs |
| 600×400 logical at 2x, direct | 1.839–1.906 ms |
| 600×400 logical at 2x, full-region backdrop blur, sigma 16 | 2.024–2.185 ms |

The measured Retina full-region blur increment was approximately 0.24 ms at the medians. The blur
path uses region extraction, separable Gaussian passes, paired linear samples, and downsampling for
large physical kernels. This is measurement evidence for this machine and workload, not a general
frame-time guarantee.

The 100-iteration allocation probe for the 256×256 sigma-16 scene recorded exactly 100 allocations
and 26,214,400 allocated bytes, all from the explicit RGBA readback vector, with matching
deallocations and no reallocations. The compositor added no per-frame CPU heap allocation after
warm-up and retained 393,216 bytes of bounded scene/scratch textures. Scenes without effects
reported zero retained compositor bytes.

The corresponding 100-iteration subtree-filter probe also recorded exactly the 100 explicit
readback allocations and no additional per-frame CPU allocation. One 256×256 filtered layer
retained 655,360 bytes across the bounded scene, scratch pair, and isolated content layer.
