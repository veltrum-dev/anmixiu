#![forbid(unsafe_code)]

mod component;
mod element;
mod scheduler;
mod shared_string;
mod state;
mod typography;

pub use component::{ComponentHost, Context, Render, RenderError, RenderOnce};
pub use element::{
    AlignItems, ButtonElement, ClickHandler, Color, CursorStyle, DivElement, Element, ElementId,
    ElementNode, FlexDirection, FluentBuilder, GlobalElementId, HitNode, HoverHandler,
    InteractiveElement, IntoClickHandler, IntoElement, IntoHoverHandler, JustifyContent, NodeId,
    ParentElement, Pixels, ScrollHandle, Stateful, StatefulInteractiveElement, Style,
    StyleRefinement, Styled, TextElement, button, div, px, text,
};
pub use scheduler::{FrameBatcher, FrameLoopError, WindowId};
pub use shared_string::SharedString;
pub use state::{AppStateStore, State, WindowStateStore};
pub use typography::Typography;

/// Well-known local endpoint used by the optional Anmixiu Dev Tools discovery agent.
#[cfg(feature = "devtools")]
pub const DEVTOOLS_SOCKET_PATH: &str = "/tmp/anmixiu-devtools.sock";
