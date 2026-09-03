> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.

# Anmixiu

Anmixiu is an experimental cross-platform native GUI runtime written in Rust. Its portable core
uses ordinary chainable Rust values, fine-grained `Signal` dependency tracking, frame-batched
updates, and Taffy Flexbox layout. The current MVP ships native macOS and Windows backends: macOS
uses AppKit, CoreText, and Metal, while Windows uses Win32, DirectWrite, Direct2D, and a D3D11/DXGI
swap chain. These are platform adapters rather than the product boundary. The longer-term platform
vision includes Linux, FreeBSD, iOS, and Android behind the same platform-neutral contracts. It
does not use a WebView, winit, GPUI, JSX, or RSX.

## Development status

> [!CAUTION]
> Anmixiu is evolving rapidly and should be considered experimental. The public API, internal
> architecture, and platform contracts may change in backwards-incompatible ways before the
> project reaches a stable release. Pin a specific version or commit for reproducible builds, and
> expect to update application code when upgrading.

## Cross-platform platform vision

Anmixiu is designed for one Rust UI model that can target desktop and mobile platforms. Shared
elements, layout, state, scheduling, and scene contracts stay platform-neutral, while each target
provides its own native window, input, text, and renderer integration. macOS and Windows are the
implemented MVP backends in this repository. Linux and FreeBSD are planned desktop backends; iOS
and Android are planned mobile backends that will reuse the same core contracts rather than fork
the public UI model. Platform-specific capabilities will remain explicit and target-gated as each
backend lands.

```rust
use anmixiu::prelude::*;

div()
    .width(320.0)
    .padding(20)
    .gap(12.0)
    .background(0x1A_1F_2E)
    .child(text("Hello"))
    .child(button("Increment").id("increment").on_click(|| {}))
```

Colors support normalized channels and compile-time hexadecimal literals:

```rust
let opaque = Color::hex(0x33_66_FF);                     // 0xRRGGBB
let translucent = Color::hex_with_alpha(0x33_66_FF_80); // 0xRRGGBBAA
```

Color-taking builders accept `impl Into<Color>`, so a 24-bit RGB literal can be passed directly:

```rust
div()
    .background(0x12_34_56)
    .foreground(0xFF_FF_FF)
    .border_color(0x65_43_21);
```

Eight-digit values are intentionally explicit: use
`.background(Color::hex_with_alpha(0x12_34_56_80))` so leading-zero RGBA values cannot be
misinterpreted as RGB.

Numeric values passed to length-taking builders default to logical pixels:

```rust
div().width(320.0).height(200).padding(16).rounded(8.0)
```

`px(320.0)` returns the concrete `Pixels` unit and remains available when an explicit unit improves
readability. Future non-pixel units will use explicit constructors and will not change the meaning
of bare numeric values.

Backdrop blur is a paint-only effect. Its Gaussian sigma is expressed in logical pixels and applies
only to content already painted behind the element; the element's own background, border, text, and
children remain sharp. A translucent fill is typically layered over the filtered backdrop:

```rust
div()
    .backdrop_blur(16.0)
    .background(Color::rgba(0.12, 0.14, 0.18, 0.55))
    .rounded(12.0)
```

Portable renderers clamp sigma to 64 logical pixels. Scenes without backdrop effects retain the
direct-to-surface fast path and allocate no compositor textures. Blur scenes use bounded reusable
intermediate textures; smaller effect bounds and static backdrops remain the most efficient usage.

Run the interactive comparison example and toggle the effect on and off:

```sh
cargo run --example backdrop_blur
```

`div()`, `text()`, and `button()` return concrete `DivElement`, `TextElement`, and
`ButtonElement` values. Custom element recipes implement `Element`; persistent stateful components
implement `Render`. Style, children, identity, and stateful interaction are separate traits exposed
through the prelude.

Built-in controls are usable by default. A button already has a visible neutral background,
readable label, intrinsic content width, centered text, one-pixel border, hover feedback, pointer
cursor, focus ring, padding, rounded corners, and a minimum hit-target height; apply `Styled` or
`InteractiveElement::hover` only to override that baseline.

Future reusable controls will be registered through an application-owned typed registry and will
still compose ordinary Rust elements. Anmixiu will not use a global string-tag namespace or a
runtime property parser.

Application typography provides defaults for all windows, and each window can independently
override the family or logical size:

```rust
App::new()
    .font_family("Avenir Next")
    .font_size(px(15.0))
    .window(Window::new().font_size(px(17.0)));
```

If neither level sets a field, the native platform UI font and its default visible size are used.

Window titles inherit the application name unless explicitly set. `Window` is the creation
configuration, while components receive live `AppHandle` and `WindowHandle` values through their
`Context`. `cx.window()` always identifies the window that owns that component;
`cx.app().active_window()` instead follows native focus.

```rust
impl Render for Workspace {
    fn render(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.app();
        button("Open inspector")
            .id("open-inspector")
            .on_click(move || {
                if let Err(error) = app.open_window(
                    Window::new().title("Inspector").size(420.0, 300.0),
                    Inspector::default(),
                ) {
                    eprintln!("failed to open inspector: {error}");
                }
            })
    }
}
```

`WindowHandle::info()` is a reactive snapshot containing the resolved title, logical content size,
native scale, focus, visibility, presentation mode, and lifecycle status. Native properties are
updated in one command with `WindowUpdate`; resetting a title restores application-name
inheritance rather than conflating reset with an empty title.

```rust
let window = cx.window();
let info = window.info();
if let Err(error) = window.update(
    WindowUpdate::new()
        .title("Renamed")
        .content_size(800.0, 600.0),
) {
    eprintln!("failed to update {:?}: {error}", info.id);
}
```

Run the native Counter on macOS or Windows:

```sh
cargo run --example counter
```

This exercises the first platform backend; additional desktop and mobile runners will be added as
their native integrations land.

Run the Counter with Anmixiu Dev Tools discovery enabled:

```sh
cargo run --features devtools --example counter
```

Run the two-axis smooth-scroll demo (120 rows with a wide horizontal surface):

```sh
cargo run --example scroll
```

Run the typed `Eventful` capability demo. It shows three Window-scope subscriptions dispatched by
priority (`high → normal → low`) and the live subscription count:

```sh
cargo run --example event
```

`Eventful` is an explicit root capability: launch an implementing component with
`App::run_eventful`. Ordinary `App::run` does not inspect or bind optional traits.

Run the multi-window example to open, update, inspect, focus, and close independently owned native
windows:

```sh
cargo run --example multi_window
```

`ScrollHandle` supports both `offset_x` and `offset_y`. A scroll container accumulates trackpad
deltas into a target and follows it on the display link, while the scene paints an unobtrusive
overlay scrollbar for each overflowing axis.

For a browser-side baseline, open [browser-reference/index.html](browser-reference/index.html). It
contains native HTML X/Y overflow, wheel/trackpad and scrollbar interactions, smooth programmatic
scroll buttons, and a small set of default element/control samples. Its [README](browser-reference/README.md)
explains how to serve the directory and where to add future comparisons.

See [docs/architecture.md](docs/architecture.md) for crate boundaries and runtime invariants.

## License

Anmixiu is distributed under the [MIT License](LICENSE).

Commercial and open-source distributors should also review the
[legal and distribution notes](https://github.com/veltrum-dev/anmixiu/blob/main/docs/legal.md).
Contributors should follow the
[source-provenance requirements](https://github.com/veltrum-dev/anmixiu/blob/main/CONTRIBUTING.md).
