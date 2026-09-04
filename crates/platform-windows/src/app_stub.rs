use anmixiu_core::{AppEvents, AppStateStore, Element, Pixels, SharedString, Typography, Window};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("the Anmixiu Windows backend requires Windows")]
    UnsupportedPlatform,
}

pub struct App {
    name: SharedString,
    state: AppStateStore,
    window: Window,
    typography: Typography,
    events: AppEvents,
}

impl Default for App {
    fn default() -> Self {
        Self {
            name: SharedString::new_static("Anmixiu"),
            state: AppStateStore::new(),
            window: Window::new(),
            typography: Typography::new(),
            events: AppEvents::new(),
        }
    }
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
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = name.into();
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
    pub fn run<C: Element>(self, _root: C) -> Result<(), AppError> {
        Err(AppError::UnsupportedPlatform)
    }
}
