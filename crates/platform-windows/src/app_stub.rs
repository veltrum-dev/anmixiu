use anmixiu_core::{
    AppEvents, AppStateStore, Eventful, Pixels, Render, Typography, WindowStateStore,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("the Anmixiu Windows backend requires Windows")]
    UnsupportedPlatform,
}

#[derive(Default)]
pub struct Window {
    state: WindowStateStore,
    typography: Typography,
}

impl Window {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn title(self, _title: impl Into<String>) -> Self {
        self
    }

    #[must_use]
    pub fn size(self, _width: f32, _height: f32) -> Self {
        self
    }

    #[must_use]
    pub fn with_state<T: 'static>(mut self, state: T) -> Self {
        self.state = self.state.with(state);
        self
    }

    #[must_use]
    pub fn font_family(mut self, family: impl Into<anmixiu_core::SharedString>) -> Self {
        self.typography = self.typography.with_font_family(family);
        self
    }

    #[must_use]
    pub fn font_size(mut self, size: impl Into<Pixels>) -> Self {
        self.typography = self.typography.with_font_size(size);
        self
    }
}

#[derive(Default)]
pub struct App {
    state: AppStateStore,
    window: Window,
    typography: Typography,
    events: AppEvents,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_state<T: 'static>(mut self, state: T) -> Self {
        self.state = self.state.with(state);
        self
    }

    #[must_use]
    pub fn window(mut self, window: Window) -> Self {
        self.window = window;
        self
    }

    #[must_use]
    pub fn font_family(mut self, family: impl Into<anmixiu_core::SharedString>) -> Self {
        self.typography = self.typography.with_font_family(family);
        self
    }

    #[must_use]
    pub fn font_size(mut self, size: impl Into<Pixels>) -> Self {
        self.typography = self.typography.with_font_size(size);
        self
    }

    #[must_use]
    pub fn events(&self) -> AppEvents {
        self.events.clone()
    }

    /// Reports that the Win32 host is unavailable on this target.
    ///
    /// # Errors
    ///
    /// Always returns [`AppError::UnsupportedPlatform`].
    pub fn run<C: Render>(self, _root: C) -> Result<(), AppError> {
        Err(AppError::UnsupportedPlatform)
    }

    pub fn run_eventful<C: Render + Eventful>(self, _root: C) -> Result<(), AppError> {
        Err(AppError::UnsupportedPlatform)
    }
}
