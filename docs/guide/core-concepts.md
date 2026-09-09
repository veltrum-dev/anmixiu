# Core concepts

## Elements are ordinary Rust values

Every public UI value implements `Element: Styled + Lifecycle`. Built-ins use concrete primitive
types, while custom Elements keep independent mounted identities and reactive owners when used as
children.

Capabilities remain separate:

- `Styled` owns style builders.
- `ParentElement` owns child builders.
- `InteractiveElement` owns semantic identity.
- `StatefulInteractiveElement` owns handlers that span input phases.

This keeps a text node from pretending to support capabilities it does not need.

## Identity is semantic

`ElementId` is supplied by the application. Calling `.id(...)` changes a concrete element to
`Stateful<E>` and gives interactions a stable identity across frames. Dense layout, paint, and hit
test indices stay internal.

Without an explicit ID, mounted identity is derived from the parent, sibling position, and Rust
type. Reconfiguring the same identity does not mount it again.

## Signals schedule frame-batched updates

A `Signal` read subscribes only while an Element is rendering inside its observer. A write changes
the value, marks each dependent mounted owner dirty, and requests a frame. It does not scan the
Element tree or render synchronously.

```rust
let count = Signal::new(0_u32);
let value = count.get();
count.set(value + 1);
```

Unmounting an owner removes its subscriptions and cancels its unfinished owner-bound UI tasks.

## Lifecycle stays synchronous

`Lifecycle::render` receives `&self`. State changes go through `Signal`; render and lifecycle hooks
do not become async. `on_mount` runs once after the first successful paint, and `on_unmount` runs
once in reverse tree order.

UI futures are spawned through `Context::spawn`. They resume on the native UI thread and remain
bound to the mounted owner that created them.

## Styles use portable values

Public `Style` belongs to `anmixiu-core`; Taffy stays behind the layout adapter. Bare `f32` and
`u32` lengths mean logical pixels, and `px(...)` returns the explicit `Pixels` type.

Colors use normalized channels or unambiguous integer formats:

```rust
let opaque = Color::hex(0x33_66_FF);
let translucent = Color::hex_with_alpha(0x33_66_FF_80);
```

Direct `u32` conversion accepts only 24-bit `0xRRGGBB`. Alpha always remains explicit.

## Native backends share one public model

macOS assembles winit, wgpu/Metal, and CoreText. Windows 10 or later assembles winit, wgpu/D3D12,
and DirectWrite. Platform-neutral crates never depend back on these implementations, and unsupported
platform-specific capabilities must not silently become no-ops.

Read the [architecture guide](../architecture) for the full dependency and update pipeline.
