# Anmixiu

Anmixiu is an experimental native macOS GUI runtime written in Rust. The MVP uses ordinary
chainable Rust values, fine-grained `Signal` dependency tracking, frame-batched updates, Taffy
Flexbox layout, CoreText shaping/rasterization, and Metal drawing. It does not use a WebView,
winit, GPUI, JSX, or RSX.

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

Run the Counter on macOS:

```sh
cargo run --example counter
```

Run the Counter with Anmixiu Dev Tools discovery enabled:

```sh
cargo run --features devtools --example counter
```

Run the two-axis smooth-scroll demo (120 rows with a wide horizontal surface):

```sh
cargo run --example scroll
```

`ScrollHandle` supports both `offset_x` and `offset_y`. A scroll container accumulates trackpad
deltas into a target and follows it on the display link, while the scene paints an unobtrusive
overlay scrollbar for each overflowing axis.

For a browser-side baseline, open [browser-reference/index.html](browser-reference/index.html). It
contains native HTML X/Y overflow, wheel/trackpad and scrollbar interactions, smooth programmatic
scroll buttons, and a small set of default element/control samples. Its [README](browser-reference/README.md)
explains how to serve the directory and where to add future comparisons.

See [docs/architecture.md](docs/architecture.md) for crate boundaries and runtime invariants.
