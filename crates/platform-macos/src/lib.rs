#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]
#![cfg_attr(target_os = "macos", allow(unsafe_code))]

mod bridge;
mod model;

#[cfg(all(feature = "devtools", target_os = "macos"))]
mod devtools;

pub use bridge::{BuiltFrame, FrameBuildError, FrameBuilder};
pub use model::{DisplayCoordinator, PointerPhase, PointerTracker, Viewport};

#[cfg(all(feature = "devtools", target_os = "macos"))]
pub(crate) use devtools::DevToolsAgent;

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
pub use app::{App, AppError, Window};

#[cfg(not(target_os = "macos"))]
mod app_stub;
#[cfg(not(target_os = "macos"))]
pub use app_stub::{App, AppError, Window};
