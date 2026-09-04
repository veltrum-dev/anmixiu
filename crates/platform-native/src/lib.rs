#![forbid(unsafe_code)]

mod bridge;
mod invalidation;
mod model;

pub use bridge::{BuiltFrame, FrameBuildError, FrameBuilder};
#[doc(hidden)]
pub use invalidation::{InvalidationGuard, RunawayInvalidation};
pub use model::{DisplayCoordinator, PointerPhase, PointerTracker, Viewport};
