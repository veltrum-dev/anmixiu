#![cfg_attr(not(target_os = "windows"), forbid(unsafe_code))]
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

pub use anmixiu_core::Window;
pub use anmixiu_platform_native::{
    BuiltFrame, DisplayCoordinator, FrameBuildError, FrameBuilder, PointerPhase, PointerTracker,
    Viewport,
};

#[cfg(target_os = "windows")]
mod app;
#[cfg(target_os = "windows")]
pub use app::{App, AppError};

#[cfg(not(target_os = "windows"))]
mod app_stub;
#[cfg(not(target_os = "windows"))]
pub use app_stub::{App, AppError};
