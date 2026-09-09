#![forbid(unsafe_code)]

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod app;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod app_stub;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod bridge;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod invalidation;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod layout_taffy;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod model;

pub use anmixiu_core::Window;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use app::{App, AppError};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use app_stub::{App, AppError};

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use bridge::{BuiltFrame, FrameBuildError, FrameBuilder};
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[doc(hidden)]
pub use invalidation::InvalidationGuard;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use layout_taffy::{
    Align, AvailableLength, Dimension, Edges, FlexDirection, Justify, LayoutCacheStats,
    LayoutEngine, LayoutError, LayoutNode, LayoutNodeId, LayoutRequest, LayoutRevisions,
    LayoutStyle, LayoutTree, LayoutViewport, MeasureId,
};
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use model::{DisplayCoordinator, PointerPhase, PointerTracker, Viewport};
