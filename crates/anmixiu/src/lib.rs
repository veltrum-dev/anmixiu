#![forbid(unsafe_code)]

//! Cross-platform native Rust UI primitives.
//!
//! The elements, layout, state, and scheduling contracts are platform-neutral. The repository's
//! complete `App`/`Window` integrations currently target macOS and Windows. Future desktop and
//! mobile adapters, including iOS and Android, are intended to reuse this same public UI model.
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

pub use anmixiu_core::{
    AlignItems, AppEvents, AppHandle, ButtonElement, ClickHandler, Color, Context, CursorStyle,
    DivElement, Element, ElementId, ElementNode, EventBindings, EventContext, EventError,
    EventPriority, EventScope, EventSubscriptionInfo, FlexDirection, FluentBuilder,
    GlobalElementId, HoverHandler, InteractiveElement, IntoClickHandler, IntoElement,
    IntoHoverHandler, JustifyContent, Lifecycle, ParentElement, Pixels, PropertyUpdate,
    RenderError, ScrollHandle, SharedString, SpawnError, State, Stateful,
    StatefulInteractiveElement, Style, StyleRefinement, Styled, Subscription, TextElement,
    Typography, Window, WindowError, WindowHandle, WindowId, WindowInfo, WindowMode, WindowSize,
    WindowStatus, WindowUpdate, WindowVisibility, button, div, px, shared_format, text,
};
pub use anmixiu_reactive::Signal;

#[cfg(target_os = "macos")]
pub use anmixiu_platform_macos::{App, AppError};
#[cfg(target_os = "windows")]
pub use anmixiu_platform_windows::{App, AppError};

pub mod prelude {
    pub use crate::{
        AlignItems, App, AppEvents, AppHandle, ButtonElement, Color, Context, CursorStyle,
        DivElement, Element, ElementId, EventBindings, EventContext, EventPriority, EventScope,
        FlexDirection, FluentBuilder, GlobalElementId, InteractiveElement, IntoElement,
        JustifyContent, Lifecycle, ParentElement, Pixels, PropertyUpdate, ScrollHandle,
        SharedString, Signal, SpawnError, State, Stateful, StatefulInteractiveElement, Style,
        StyleRefinement, Styled, Subscription, TextElement, Typography, Window, WindowError,
        WindowHandle, WindowInfo, WindowMode, WindowSize, WindowStatus, WindowUpdate,
        WindowVisibility, button, div, px, shared_format, text,
    };
}
