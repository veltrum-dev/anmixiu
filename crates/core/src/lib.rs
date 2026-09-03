#![forbid(unsafe_code)]

mod component;
mod element;
mod events;
mod scheduler;
mod shared_string;
mod state;
mod typography;
mod window;

pub use component::{ComponentHost, Context, Eventful, Render, RenderError, RenderOnce};
pub use element::{
    AlignItems, ButtonElement, ClickHandler, Color, CursorStyle, DivElement, Element, ElementId,
    ElementNode, FlexDirection, FluentBuilder, GlobalElementId, HitNode, HoverHandler,
    InteractiveElement, IntoClickHandler, IntoElement, IntoHoverHandler, JustifyContent, NodeId,
    ParentElement, Pixels, ScrollHandle, Stateful, StatefulInteractiveElement, Style,
    StyleRefinement, Styled, TextElement, button, div, px, text,
};
pub use events::{
    AppEvents, EventBindings, EventContext, EventError, EventPriority, EventScope,
    EventSubscriptionInfo, MAX_EVENTS_PER_DISPATCH, MAX_PENDING_EVENTS, Subscription,
};
pub use scheduler::{FrameBatcher, FrameLoopError, WindowId};
pub use shared_string::SharedString;
pub use state::{AppStateStore, State, WindowStateStore};
pub use typography::Typography;
pub use window::{
    AppHandle, ErasedComponentHost, MountedWindowRoot, PropertyUpdate, Window, WindowAction,
    WindowDispatcher, WindowError, WindowHandle, WindowInfo, WindowMode, WindowMountContext,
    WindowParts, WindowRoot, WindowSize, WindowStatus, WindowUpdate, WindowVisibility,
};

/// Well-known local endpoint used by the optional Anmixiu Dev Tools discovery agent.
#[cfg(feature = "devtools")]
pub const DEVTOOLS_SOCKET_PATH: &str = "/tmp/anmixiu-devtools.sock";
