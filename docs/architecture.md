# Architecture

## Dependency direction

`anmixiu` is the thin public facade. `anmixiu-core` owns elements, components, public `Style`,
events, state lookup, lifecycle, and scheduling contracts. `anmixiu-reactive` and
`anmixiu-scene` are platform-neutral leaves. `anmixiu-runtime` adds Tokio and owner-bound local UI
future scheduling. `anmixiu-layout-taffy` is an internal adapter from a projection of core styles
to Taffy Flexbox. `anmixiu-platform-native` owns the shared element-to-layout/scene projection and
portable input/display models, selecting the native text implementation at compile time.
`anmixiu-render-metal`/`anmixiu-text-coretext` and
`anmixiu-render-d3d11`/`anmixiu-text-directwrite` consume platform-neutral scene and geometry data.
`anmixiu-platform-macos` assembles AppKit while `anmixiu-platform-windows` assembles Win32. Future
desktop and mobile backends, including iOS and Android, will plug into the same contracts with
target-specific windowing, input, text, and rendering implementations.

Dependencies always point from platform implementations toward contracts. Core crates never know
about AppKit, Win32, Metal, D3D11, CoreText, DirectWrite, or a concrete event loop. Taffy types are
not part of the public element or style API.

Native FFI remains confined to the six implementation crates: `anmixiu-platform-macos`,
`anmixiu-render-metal`, `anmixiu-text-coretext`, `anmixiu-platform-windows`,
`anmixiu-render-d3d11`, and `anmixiu-text-directwrite`. Every shared contract and the facade forbid
unsafe Rust.

## Update pipeline

Within an explicit component render observer, reading a `Signal` records one owner/source edge.
A write mutates the value and inserts each live dependent owner into a deduplicated dirty queue; it
never renders inline. The window requests one display turn. That turn removes redundant descendants
when an already-dirty ancestor covers them, rerenders only remaining owners, and reuses layout and
scene cache entries whose complete revision keys still match. A clean turn submits no GPU work, and
a normal display turn presents at most one scene snapshot. Invalidations raised during render are moved
to the next turn and guarded against an infinite render loop.

Unmount removes dependency edges and cancels unfinished owner-bound UI futures. Application and
window state are retained only by their corresponding stores; same-typed window state takes
precedence over application state.

## Element identity

`ElementId` is a caller-provided semantic identity, not a traversal index. Calling `.id(...)`
changes a concrete builder such as `ButtonElement` to `Stateful<ButtonElement>`; APIs that require state across
multiple input phases, including `.on_click(...)`, are available only on that stateful wrapper.
The wrapper is erased by `IntoElement`, while its ID remains in the element tree.

The platform combines named ancestor IDs into `GlobalElementId`. Hit regions keep their dense
frame-local `HitId` for renderer efficiency but map back to this semantic path, so inserting an
unrelated sibling between mouse-down and mouse-up cannot retarget the click. Duplicate semantic
paths in one rendered tree are a structured build error. Taffy `LayoutNodeId` values remain private
frame-local indices and are never exposed as application identity.

## Shared values and conditional builders

Custom elements implement `Element`; persistent components implement `Render`. Built-ins are
concrete `DivElement`, `TextElement`, and `ButtonElement` values. `Styled`, `ParentElement`,
`InteractiveElement`, and `StatefulInteractiveElement` isolate style, tree, identity, and handler
capabilities. The heterogeneous `ElementNode` projection is doc-hidden and exists only because a
single child vector must hold different concrete element types across crate boundaries.

Built-ins have minimal usable defaults rather than being aliases for a generic node. `DivElement`
defaults to a neutral Flex column container, `TextElement` inherits foreground and uses native text
metrics, and `ButtonElement` supplies a visible neutral background, white label,
36-pixel minimum height, padding, one-pixel border, hover refinement, an 8-pixel radius, intrinsic
cross-axis sizing, centered label placement, pointer cursor, and a two-pixel focus ring.
Borders are paint-only inset layers; hover refinements can change background, foreground, and
border color without invalidating Taffy layout. AppKit tracking areas and Win32 mouse-leave tracking
clear hover when the pointer exits the native view. `Styled` overrides remain authoritative; brand
variants and themes belong to a future
component layer.

Application and window typography are optional, field-wise defaults. A window font family or size
overrides the matching application field while leaving the other field free to fall back. When
neither level specifies a field, the platform supplies its native UI font and a visible default
size: CoreText resolves the macOS default while Windows reads the current non-client message/UI
font family and logical size for DirectWrite. Windows refreshes those derived values after a system
settings change and invalidates text, layout, and scene results that embed the old metrics. A
literal zero-sized computed font is never produced by omission.

Colors support normalized floating-point `rgb`/`rgba` constructors and const integer
`hex(0xRRGGBB)` / `hex_with_alpha(0xRRGGBBAA)` constructors. Separate functions avoid ambiguity
for leading-zero values and keep string parsing out of render/style hot paths. `Styled` and
`StyleRefinement` color setters accept `impl Into<Color>`; direct `u32` values are strictly
`0xRRGGBB`, while alpha remains explicit through `hex_with_alpha`.

`px(...)` returns the concrete `Pixels` unit. Pixel-taking builders accept `impl Into<Pixels>`;
bare `f32`/`u32` values remain logical pixels, so `.width(320.0)` is equivalent to
`.width(px(320.0))`. Future percentage or relative units use distinct concrete types and named
constructors rather than making `px(...)` return an erased unit container.

`SharedString` is the public immutable string for element text, button labels, and named
`ElementId` variants. It wraps `SmolStr`: static values are borrowed, short values are inline, and
long values are shared by clones. `shared_format!` formats directly into this storage. Rendered
element trees are retained as main-thread `Rc<ElementNode>` snapshots so platform layout/paint does not
deep-clone the tree.

`FluentBuilder` provides `when`, `when_else`, `when_some`, and `when_none`. These methods preserve
the concrete builder type and conditionally apply a closure, avoiding an erased conditional-element
type while supporting optional subtrees, styles, and handlers.

The future extension point for reusable custom controls is a typed component registry layered above
the element builders. A registration associates a Rust component constructor and explicit metadata
(name, version, supported properties, and lifecycle owner) with an application-owned registry; it
does not parse tag names, mutate a global namespace, or introduce a shadow DOM. Components still
return ordinary `Element` values and retain the same Signal/owner/lifecycle contracts. This gives
library authors discoverability and namespacing without reproducing a stringly typed global element
registry.

## Async boundary

Each application owns one Tokio multithread runtime for timers and I/O readiness. UI futures use a
bounded `async-task` queue and are polled only by the active platform's native UI thread: AppKit's
main thread on macOS and the HWND-owning thread on Windows. Future backends provide an equivalent
native UI executor without changing the owner contract. `Context::spawn` binds a future to the
current persistent component owner, so callers do not retain or detach a task handle. The Tokio
runtime uses two workers: enough to remain multithreaded when one I/O task is delayed, without
scaling idle UI thread count to every logical CPU. Lifecycle methods and render remain synchronous.

## Cache contracts

- Layout: keyed by root identity, structure/style/measure revisions, logical viewport, and scale;
  one current entry per engine.
- Scene: keyed by node, paint/layout revisions, and scale; bounded LRU capacity.
- Glyph atlas: keyed by font identity, size, scale, glyph, and quantized subpixel phase;
  fixed page dimensions and entry capacity, with generation changes forcing texture refresh.
- Renderer resources: Metal retains bounded pipelines, atlas textures, and staging buffers; D3D11
  retains a hard-capacity LRU of Direct2D A8 atlas bitmaps keyed by atlas id and generation.

## Native scale and refresh

macOS frame requests are coalesced onto the `NSView` display link rather than drawn immediately by
the main dispatch queue. The link follows the window's current display, so the built-in 120 Hz
Retina panel and a 60 Hz external panel drive different tick rates without submitting more than one
ordinary frame per tick. Every tick asks `NSView::convertSizeToBacking` for the native backing size
rather than assuming `logical_size * backingScaleFactor`; logical layout size, DPR, and exact
physical surface size are tracked separately. The view opts into inherited layer `contentsScale`
and redraw-during-resize behavior.

`MetalRenderer` remembers the configured physical surface size. A drawable left over from the old
display pool is returned as `SurfaceOutOfDate` and never submitted with a new-scale Scene; the next
display tick retries after `CAMetalLayer` catches up.
Live resize stores the newest logical/backing viewport and applies `drawableSize` once at the draw
boundary, rather than mutating the layer for every `setFrameSize:` callback. Layer presentation is
transaction-coordinated and waits only until the command buffer is scheduled, so Core Animation does
not stretch an older drawable while the next frame is being queued and the AppKit thread does not
wait for GPU completion.
The first frame is also attempted synchronously after the view and layer are attached, because a
display-link callback may not have entered the active run-loop mode yet. A transient unavailable
drawable gets one follow-up display turn; repeated misses do not spin and remain recoverable on the
next resize or external wake.

Text placement uses a position-aware native glyph cache. Layout completes before rasterization; the
final glyph position at the active DPR selects one of four horizontal subpixel mask variants.
Geometry uses a physical-pixel `floor(x)` / `round(y)` origin while the chosen mask preserves the
fractional CoreText advance, so Metal samples atlas texels one-to-one without destroying kerning.
Vertical placement rounds the shared line baseline before applying each glyph's integer bearing;
individual glyph tops are never rounded independently, so mixed scripts and fallback fonts remain
on one baseline.
CoreText rasterizes into an RGB32 CGContext, honoring platform antialiasing/smoothing, and the text
backend extracts a renderer-independent A8 mask. Glyph UVs/quads retain a transparent
two-pixel safety border so low-DPI coverage is not cropped. Scale and final positioned origin are
part of the bounded text/atlas cache contracts.

Windows opts into Per-Monitor-V2 DPI awareness before creating its HWND. Client rectangles remain
integer physical pixels, while `GetDpiForWindow` derives the logical viewport used by layout and
input. `WM_DPICHANGED` applies the system-suggested outer rectangle, and every size/scale transition
unbinds the old Direct2D target before resizing the DXGI buffers and rebuilding the exact-size
target. Stale physical size or scale is reported as `SurfaceOutOfDate` instead of presenting a
scene against mismatched coordinates.

Windows frame requests are deduplicated through private window messages and a single armed frame
timer. UI-runtime wakes, component invalidations, hover changes, scroll animation, resize, and
paint exposure all converge on that path. Pointer coordinates are converted from physical client
pixels to logical pixels; button capture preserves down/up delivery, and wheel messages preserve
signed coordinates on monitors with negative desktop origins.

DirectWrite shapes complete text layouts so script fallback, bidirectional ordering, and glyph
advances are supplied by the native engine. Per-run font-file identity, face index, simulation,
em size, scale, glyph id, and quantized X/Y subpixel phase form the bounded atlas key. DirectWrite
produces ClearType coverage, which the backend reduces to a renderer-independent A8 mask with a
transparent two-pixel border. Direct2D uploads that page as `DXGI_FORMAT_A8_UNORM` with a supported
premultiplied alpha mode and draws it only as an opacity mask.

## Future platforms

There are deliberately no placeholder crates or public APIs for unsupported operating systems.
Linux and FreeBSD will compile Wayland and X11 support together and select at runtime, preferring
Vulkan with GL fallback. The longer-term mobile roadmap includes native iOS and Android
integrations. Those backends should reuse the platform-neutral `anmixiu-core`,
`anmixiu-reactive`, `anmixiu-scene`, and `anmixiu-runtime` contracts while mapping window and
lifecycle, input, text, and rendering work to each platform's native APIs.
