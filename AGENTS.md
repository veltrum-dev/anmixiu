# Anmixiu engineering contract

The repository root `.rules` file is a supplementary, high-signal Rust and cross-platform API
contract. Read and follow it together with this document; do not replace this engineering contract
with `.rules`.

## Product and public API

- Anmixiu is a native, pure-Rust GUI workspace. The public surface uses ordinary Rust values and chainable builders (`div().height(px(40.)).child(text("Hello"))`); do not add JSX, RSX, tag macros, WebView, GPUI, or winit.
- `anmixiu::Context` is the component context name. `Render::render` receives `&self`; state mutation goes through `Signal`. Lifecycle callbacks are synchronous. `RenderOnce` consumes `self` and has no lifecycle.
- Custom element values implement the public `Element` trait; persistent components implement `Render`. Built-ins return concrete `DivElement`, `TextElement`, and `ButtonElement` types. Heterogeneous storage uses the doc-hidden `ElementNode` projection only across internal crate boundaries.
- Public `ElementId` is a caller-provided semantic identity. `.id(...)` upgrades a concrete element to `Stateful<E>` and unlocks stateful interaction APIs such as `.on_click(...)`; dense layout/paint node indices remain internal implementation details.
- Element capabilities follow interface segregation: `Styled` owns style builders, `ParentElement` owns child builders, `InteractiveElement` owns identity, and `StatefulInteractiveElement` owns stateful handlers. Do not add these methods as inherent methods on a universal element type.
- Built-in concrete elements must be useful without appearance boilerplate. Containers and text keep neutral layout/inheritance defaults; controls such as `ButtonElement` provide intrinsic sizing, padding, centered content, pointer cursor, focus ring, and a visible accessible-size baseline with border and hover feedback that `Styled`/`InteractiveElement` can override. Do not turn these baselines into a full theme/component library.
- Reusable custom controls may later use an application-owned typed registry. Do not add a global string/tag registry, shadow-DOM clone, or runtime property parser; registered components must compose ordinary Rust `Element` values and preserve owner/lifecycle contracts.
- Public immutable UI strings use `SharedString`, with `new_static` and `shared_format!` for allocation-free static/short hot paths. Prefer borrowed slices within a frame, `Rc` for main-thread snapshots, and `Arc` only for genuinely cross-thread immutable data; do not call every shared handle "zero-copy" without allocation evidence.
- Conditional builder changes use `FluentBuilder::{when, when_else, when_some, when_none}` rather than optional placeholder nodes.
- Public `Style` is owned by `anmixiu-core`; never expose Taffy types. Taffy is an unconditional internal Flexbox implementation dependency.
- `Color` accepts normalized `rgb`/`rgba` channels and explicit const integer formats: `hex(0xRRGGBB)` and `hex_with_alpha(0xRRGGBBAA)`. Color-taking builders accept `impl Into<Color>`; `u32 -> Color` means strict 24-bit `0xRRGGBB` only. Do not guess alpha from numeric magnitude or parse strings in style hot paths.
- `px(...)` returns concrete `Pixels`; pixel-taking builders accept `impl Into<Pixels>`. Bare `f32` and `u32` mean logical pixels. Future non-pixel units require distinct concrete types and named constructors and must not change this default.
- Tailwind-style aliases may only be incremental feature-gated conveniences. Long-form builders remain available.

## Crates and dependency direction

- `anmixiu` is a thin facade.
- Platform-neutral leaves: `anmixiu-reactive`, `anmixiu-scene`.
- `anmixiu-runtime` depends on reactive owner contracts; `anmixiu-core` depends on reactive, scene, and runtime contracts; `anmixiu-layout-taffy` adapts core styles; platform renderers consume scene commands.
- `anmixiu-platform-macos` owns AppKit input/window assembly and may depend on core, layout, runtime, Metal, and CoreText implementations. Core crates never depend back on platform implementations.
- Third-party versions and internal paths belong in root `[workspace.dependencies]`; member crates inherit them. OS implementations use target-specific dependencies, never OS-selection features.
- Do not add empty Windows/Linux/FreeBSD crates. Future desktop direction: Windows uses Win32 + D3D11 + DirectWrite; Linux/FreeBSD compile Wayland and X11 together with runtime selection, preferring Vulkan and falling back to GL. The longer-term mobile direction includes native iOS and Android backends that reuse the platform-neutral contracts; add those crates only with a real implementation.

## Test-driven development

- After the minimal compiling workspace skeleton, every behavior change follows Red -> Green -> Refactor: add a focused failing test, observe the intended failure, implement the smallest correct behavior, then clean up without weakening assertions.
- Never hide races with arbitrary sleeps, swallow errors, or replace precise assertions with smoke tests. Public trait implementations require contract tests.
- Before handoff run formatting, check, clippy with warnings denied, unit/integration/doc tests, the Counter example, release benchmarks, and macOS offscreen rendering where supported. State anything not verified.

## External references

- External frameworks and engineering reports may be used only to discover problem classes,
  tradeoffs, and validation scenarios. Do not copy their code, internal names, or architecture into
  Anmixiu. Derive independent contracts from Anmixiu's requirements, and require tests and benchmark
  evidence when claiming an improvement over a known failure mode.

## Runtime and performance

- Signal reads subscribe only inside an explicit render observer. Writes mutate and mark owners dirty; rendering is frame-batched and deduplicated. Unmount removes subscriptions and cancels owner tasks.
- Each app owns one minimal-feature Tokio multithread runtime. UI futures are local, resume only on the AppKit main thread, and are owner-bound. Do not make lifecycle or render async.
- Render, layout, paint, and input hot paths contain no blocking I/O, parsing, ordinary business-field mutation, or unbounded task/observer/history creation.
- Every cache documents its key, invalidation rule, and hard capacity. Warm steady-state memory must not grow continuously. Algorithmic performance changes need before/after benchmark evidence.
- Frame reference targets are 8.33 ms at 120 Hz and 16.67 ms interaction-to-next-frame; they are engineering targets, not correctness promises.
- Keep logical pixels, scale-adjusted floating coordinates, and integer device pixels distinct at
  rendering boundaries. Fractional values require 1x/2x placement and raster tests; never give a
  sub-logical-pixel value an undocumented physical-pixel meaning.
- Do not add a frame arena merely to reduce allocation counts. First measure allocation sites, then
  prefer bounded reusable buffers or retained immutable snapshots; any arena must prove better
  lifetime ergonomics and steady-state memory with benchmarks.
- The UI thread must never wait for GPU completion. Any CPU-writable buffer visible to an in-flight
  command buffer needs an explicit ring/pool and completion-based reuse before asynchronous
  submission is introduced. Validate both composited and direct presentation modes where possible.
- macOS frame delivery follows the active `NSView` display link. Before every tick synchronize bounds/backing scale; never submit a drawable whose physical size differs from the configured surface. Glyph quads must align to the active physical pixel grid across 1x/2x scale transitions and retain a verified transparent atlas safety border for antialias coverage.

## Safety and scope

- `unsafe`/FFI is allowed only in `platform-macos`, `render-metal`, and `text-coretext`; every unsafe block or impl must have an adjacent `// SAFETY:` explanation. Core, reactive, scene, layout, runtime, facade, and examples forbid unsafe.
- Prefer structured errors for recoverable failures. Keep responsibilities narrow and compose concrete types before inventing traits or compatibility layers.
- Keep source files responsibility-focused. A module with submodules uses `module.rs` plus a sibling `module/` directory (for example `element.rs` with `element/style.rs` and `element/div.rs`); do not use `mod.rs`. Split by stable responsibility, not arbitrary line counts.
- Out of MVP scope: non-macOS backends, web, async lifecycle/render, blur, Grid/full Block, IME/input, clipboard, accessibility, images, scrolling, full themes/component libraries/Tailwind, arbitrary subtree State, and application-wide async shutdown. This is an MVP boundary rather than the product boundary; the longer-term roadmap includes Windows/Linux/FreeBSD plus native iOS and Android backends.
- Preserve user changes. Work directly on `main`; do not create worktrees, branches, or commits unless the user explicitly asks later.
