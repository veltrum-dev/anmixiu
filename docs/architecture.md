# Architecture

## Dependency direction

`anmixiu` is the thin public facade. `anmixiu-core` owns elements, components, public `Style`,
events, state lookup, lifecycle, and scheduling contracts. `anmixiu-reactive` and
`anmixiu-scene` are platform-neutral leaves. `anmixiu-runtime` adds Tokio and owner-bound local UI
future scheduling. `anmixiu-platform` owns the internal Taffy adapter, shared
element-to-layout/scene projection, portable input/display models, winit event loop, and the
connection between each native window and its wgpu surface. `anmixiu-render` consumes
platform-neutral Scene data through wgpu, which selects Metal on macOS and D3D12 on Windows.
`anmixiu-text` selects CoreText or DirectWrite at compile time. Future desktop and mobile backends
plug into the same contracts without changing the public UI model.

Dependencies always point from platform implementations toward contracts. Core crates never know
about winit, wgpu, Metal, D3D12, CoreText, DirectWrite, or a concrete event loop. Taffy types are
not part of the public element or style API.

`anmixiu-platform` and `anmixiu-render` forbid unsafe Rust and rely on their upstream window and
surface implementations. Native text FFI is confined to the target-specific modules in
`anmixiu-text`. Shared contracts and the facade forbid unsafe Rust.

## Update pipeline

Within an explicit mounted-Element render observer, reading a `Signal` records one owner/source edge.
A write mutates the value and inserts each live dependent owner into a deduplicated dirty queue; it
never scans the Element tree or renders inline. The window requests one display turn. The retained
lifecycle tree routes each dirty owner directly to its mounted Element, rerenders only matching
owners, and reuses every clean Element snapshot; layout and scene caches then
reuse entries whose complete revision keys still match. A clean turn submits no GPU work, and a
normal display turn presents at most one scene snapshot. Invalidations raised during render are moved
to the next turn and guarded against an infinite render loop.

Unmount removes dependency edges and cancels unfinished owner-bound UI futures. Application and
window state are retained only by their corresponding stores; same-typed window state takes
precedence over application state.

## Native windows

`Window` is a portable creation configuration. Its optional title means “inherit the application
name”; an explicit empty `SharedString` remains an intentionally empty title. Native adapters
resolve that configuration into a `WindowInfo` snapshot and retain it behind `WindowHandle`.
Reading the snapshot during render subscribes the mounted Element owner, so native resize, scale, focus,
visibility, and presentation-mode changes invalidate the appropriate frame without synchronous
AppKit or Win32 queries from application code.

Each application owns a bounded window-command queue and a live registry keyed by generated
`WindowId`. A command is removed from the queue before it runs, allowing mount and input callbacks
to enqueue another open/update/close operation without re-entering a `RefCell` borrow. Closed
windows are removed rather than retained as history. Their root host unmounts, owner tasks and event
subscriptions are cancelled, renderer/surface state is released, and stale handles retain only the
final `Closed` snapshot. The native application loop exits only when the final window closes.

All windows share the application's single Tokio runtime, application state, and typed event
router. Each window separately owns its root host, reactive owner registry, window state, frame
builder, wgpu surface state, viewport, and pointer state. winit routes every native event and
coalesced `RedrawRequested` through its native window id, which maps to Anmixiu's immutable
`WindowId`. `Context::window()` is therefore owner-bound and stable, while
`AppHandle::active_window()` is the changing native-focus view.

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

Every public UI value implements `Element: Styled + Lifecycle`. Built-ins are concrete
`DivElement`, `TextElement`, and `ButtonElement` primitive fast paths. A custom Element used as a
child becomes a real neutral layout box and retains its own typed lifecycle host and reactive owner;
its `Lifecycle::render` output becomes content below that box. `ParentElement`,
`InteractiveElement`, and `StatefulInteractiveElement` remain optional capabilities. The
heterogeneous `ElementNode` projection is doc-hidden and exists only across internal crate
boundaries.

Unkeyed mounted identity is `(parent, sibling position, Rust TypeId)`; `.id(...)` replaces the
position with caller-provided semantic identity. Re-rendering a parent with a new value of the same
identity updates that Element's configuration and observer without repeating mount. A successful
first paint calls `on_mount` parent-to-child and left-to-right. Removal calls `on_unmount` in the
exact reverse order. Signal writes route directly to the subscribed owner; unchanged siblings do
not execute `render`.

Built-ins have minimal usable defaults rather than being aliases for a generic node. `DivElement`
defaults to a neutral Flex column container, `TextElement` inherits foreground and uses native text
metrics, and `ButtonElement` supplies a visible neutral background, white label,
36-pixel minimum height, padding, one-pixel border, hover refinement, an 8-pixel radius, intrinsic
cross-axis sizing, centered label placement, pointer cursor, and a two-pixel focus ring.
Borders are paint-only inset layers; hover refinements can change background, foreground, and
border color without invalidating Taffy layout. winit cursor-enter/leave events clear hover when the
pointer exits a native window. `Styled` overrides remain authoritative; brand variants and themes
belong to a future component layer.

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

The optional `anmixiu` `macros` feature provides `#[derive(Element)]`. A field marked
`#[element(style)]` delegates `Styled`; adding `parent` to that marker delegates `ParentElement` as
well. The derive emits no lifecycle or interaction implementation, so `Lifecycle::render` and other
capabilities remain explicit and capability segregation is preserved.

## Typed events

`Lifecycle::bind_events` is a default no-op hook invoked once after the first frame is painted. Its
`EventBindings` value retains each RAII `Subscription` until unmount. Event payloads are ordinary
Rust values routed by `TypeId`, never string topics. `EventScope::Owner` matches the originating
mounted Element owner exactly. `EventScope::Window` restricts delivery to the current window, while
`EventScope::App` broadcasts through the App-owned router across windows.

Subscriptions carry an `EventPriority`. Higher values dispatch first; equal priorities retain
registration order. Nested emissions are queued FIFO with a hard pending capacity and a bounded
number of deliveries per synchronous turn, so a feedback loop cannot monopolize the UI thread.
Callbacks are invoked without holding the router's mutable borrow so a handler can emit or
unsubscribe safely. Panic guards restore an in-flight callback and discard its queued nested work.
Subscription cancellation also unregisters its owner cleanup immediately, keeping dynamic churn
bounded for the owner's lifetime.

The router exposes read-only subscription metadata snapshots for diagnostics; event payload values
are never retained as state.

The future extension point for reusable custom controls is a typed component registry layered above
the element builders. A registration associates a Rust component constructor and explicit metadata
(name, version, supported properties, and lifecycle owner) with an application-owned registry; it
does not parse tag names, mutate a global namespace, or introduce a shadow DOM. Components still
return ordinary `Element` values and retain the same Signal/owner/lifecycle contracts. This gives
library authors discoverability and namespacing without reproducing a stringly typed global element
registry.

## Async boundary

Each application owns one Tokio multithread runtime for timers and I/O readiness. UI futures use a
bounded `async-task` queue and are polled only by the thread running winit's native event loop.
Future backends provide an equivalent native UI executor without changing the owner contract.
`Context::spawn` binds a future to the current mounted Element owner and returns a structured
`SpawnError`, so callers do not retain
or detach a task handle and capacity/lifecycle rejection never becomes a framework panic. The Tokio
runtime uses two workers: enough to remain multithreaded when one I/O task is delayed, without
scaling idle UI thread count to every logical CPU. Lifecycle methods and render remain synchronous.

## Cache contracts

- Layout: keyed by root identity, structure/style/measure revisions, logical viewport, and scale;
  one current entry per engine.
- Scene: keyed by node, paint/layout revisions, and scale; bounded LRU capacity.
- Glyph atlas: keyed by font identity, size, scale, glyph, and quantized subpixel phase;
  fixed page dimensions and entry capacity, with generation changes forcing texture refresh. A
  frame that observes a repack is rebuilt from a single atlas generation; a frame whose glyph union
  cannot stabilize within the bounded page returns a structured error instead of submitting stale
  UVs.
- Renderer resources: wgpu retains pipeline sets plus a hard-capacity glyph-atlas LRU keyed by
  atlas id and generation. Compositor textures are replaced on physical size/format/depth changes
  and have a 256 MiB hard budget.

## Backdrop effects and compositing

`Style::backdrop_blur` is paint-only and stores a Gaussian sigma in logical pixels. The shared
projection emits an ordered `DrawCommand::BackdropBlur` immediately before the element's border and
background commands. Its semantic input is every preceding command in the current Scene; later
commands, including the element's own fill, text, and descendants, remain unfiltered. Non-positive
or non-finite style values emit no effect, and platform renderers clamp larger finite sigma values
to the shared 64-logical-pixel ceiling.

An effect command selects the compositor path. A Scene without effects keeps the existing direct
surface render pass and creates no intermediate color textures. The active wgpu renderer uses a
bounded scene texture, one reusable blur pair, and one transparent texture per nested filter layer.
It preserves Scene order, clips replacement/composite passes, caps each effect kind at 64, caps
filter nesting at eight, and rejects plans above 256 MiB before allocating GPU textures.

## Native scale and refresh

winit reports client size in exact physical pixels and reports scale changes separately. Anmixiu
derives one logical viewport from those two values, reconfigures the wgpu surface to the same
physical dimensions, and renders only from `RedrawRequested`. `Occluded` suppresses minimized or
hidden-window work. Surface timeout/occlusion skips a frame, an outdated surface is reconfigured,
and a lost surface is recreated. No Anmixiu code owns a `CAMetalLayer` or DXGI swap chain.

Text placement uses a position-aware native glyph cache. Layout completes before rasterization; the
final glyph position at the active DPR selects one of four horizontal subpixel mask variants.
Geometry uses a physical-pixel `floor(x)` / `round(y)` origin while the chosen mask preserves the
fractional native advance, so wgpu samples atlas texels one-to-one without destroying kerning.
Vertical placement rounds the shared line baseline before applying each glyph's integer bearing;
individual glyph tops are never rounded independently, so mixed scripts and fallback fonts remain
on one baseline.
CoreText rasterizes into an RGB32 CGContext, honoring platform antialiasing/smoothing, and the text
backend extracts a renderer-independent A8 mask. Glyph UVs/quads retain a transparent
two-pixel safety border so low-DPI coverage is not cropped. Scale and final positioned origin are
part of the bounded text/atlas cache contracts.

DirectWrite shapes complete text layouts so script fallback, bidirectional ordering, and glyph
advances are supplied by the native engine. Per-run font-file identity, face index, simulation,
em size, scale, glyph id, and quantized X/Y subpixel phase form the bounded atlas key. DirectWrite
produces ClearType coverage, which the backend reduces to a renderer-independent A8 mask with a
transparent two-pixel border. The renderer uploads that page as an R8 texture and samples it only as
an opacity mask.

## Future platforms

There are deliberately no placeholder crates or public APIs for unsupported operating systems.
Linux and FreeBSD will compile Wayland and X11 support together and select at runtime, preferring
Vulkan with GL fallback. The longer-term mobile roadmap includes native iOS and Android
integrations. Those backends should reuse the platform-neutral `anmixiu-core`,
`anmixiu-reactive`, `anmixiu-scene`, and `anmixiu-runtime` contracts while mapping window and
lifecycle, input, text, and rendering work to each platform's native APIs.
