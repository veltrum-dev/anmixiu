# 快速开始

Anmixiu 当前支持 macOS 和 Windows 原生应用，需要 Rust 1.89 或更高版本，并且尚未发布到
crates.io。

## 运行仓库示例

克隆仓库并运行 Counter 示例：

```sh
git clone https://github.com/veltrum-dev/anmixiu.git
cd anmixiu
cargo run --example counter
```

其他示例分别覆盖滚动、类型化事件、多窗口、模糊效果、可选的 Tailwind 风格别名，以及自定义
Element 派生：

```sh
cargo run --example scroll
cargo run --example event
cargo run --example multi_window
cargo run --example backdrop_blur
cargo run --features tailwind --example tailwind
cargo run --features macros --example custom_element
```

## 构建第一个界面

Anmixiu 使用具体 Rust 值和可链式构建器：

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

`div()`、`text()` 和 `button()` 分别返回具体的 `DivElement`、`TextElement` 和
`ButtonElement`。元素通过 `.id(...)` 升级为 `Stateful<E>` 后，才能使用有状态交互处理器。

## 运行检查

常规本地验证命令如下：

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --doc --all-features
```

接下来阅读[核心概念](./core-concepts)，了解 Element、身份、生命周期与响应式更新。
