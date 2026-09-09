# Getting started

Anmixiu currently supports native applications on macOS and Windows. It requires Rust 1.89 or
newer and is not yet published to crates.io.

## Run the repository examples

Clone the repository and run the Counter example:

```sh
git clone https://github.com/veltrum-dev/anmixiu.git
cd anmixiu
cargo run --example counter
```

Other examples exercise scrolling, typed events, multiple windows, blur effects, optional
Tailwind-style aliases, and the custom Element derive:

```sh
cargo run --example scroll
cargo run --example event
cargo run --example multi_window
cargo run --example backdrop_blur
cargo run --features tailwind --example tailwind
cargo run --features macros --example custom_element
```

## Build a first interface

Anmixiu uses concrete Rust values and chainable builders:

```rust
use anmixiu::prelude::*;

let content = div()
    .width(320.0)
    .padding(20)
    .gap(12.0)
    .background(0x1A_1F_2E)
    .child(text("Hello from Anmixiu"))
    .child(
        button("Continue")
            .id("continue")
            .on_click(|| println!("clicked")),
    );
```

`div()`, `text()`, and `button()` return concrete `DivElement`, `TextElement`, and `ButtonElement`
values. Stateful handlers become available after `.id(...)` upgrades an element to `Stateful<E>`.

## Run the checks

The normal local verification commands are:

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

Continue with [core concepts](./core-concepts) to understand Elements, identity, lifecycle, and
reactive updates.
