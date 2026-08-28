#![forbid(unsafe_code)]

//! Native Rust UI primitives for macOS.
//!
//! ```
//! use anmixiu::prelude::*;
//!
//! let card = div()
//!     .width(px(320.0))
//!     .padding(px(16.0))
//!     .gap(px(8.0))
//!     .background(Color::rgb(0.1, 0.12, 0.18))
//!     .child(text("Hello from Anmixiu"));
//! assert_eq!(card.children_ref().len(), 1);
//! ```

pub use anmixiu_core::*;
pub use anmixiu_reactive::Signal;

#[cfg(target_os = "macos")]
pub use anmixiu_platform_macos::{App, AppError, Window};

pub mod prelude {
    pub use crate::{
        AlignItems, App, ButtonElement, Color, Context, CursorStyle, DivElement, Element,
        ElementId, FlexDirection, FluentBuilder, GlobalElementId, InteractiveElement, IntoElement,
        JustifyContent, ParentElement, Pixels, Render, RenderOnce, ScrollHandle, SharedString, Signal,
        State, Stateful, StatefulInteractiveElement, Style, StyleRefinement, Styled, TextElement,
        Typography, Window, button, div, px, shared_format, text,
    };
}
