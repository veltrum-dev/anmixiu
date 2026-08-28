use crate::{Pixels, SharedString};

/// Application- or window-level typography defaults.
///
/// Missing fields remain unresolved so the platform can supply its native UI defaults.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Typography {
    font_family: Option<SharedString>,
    font_size: Option<Pixels>,
}

impl Typography {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            font_family: None,
            font_size: None,
        }
    }

    #[must_use]
    pub fn with_font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    #[must_use]
    /// Sets an explicit positive logical font size.
    ///
    /// # Panics
    ///
    /// Panics unless the size is finite and greater than zero. Omit the field to request the
    /// platform default size.
    pub fn with_font_size(mut self, size: impl Into<Pixels>) -> Self {
        let size = size.into();
        assert!(size.value().is_finite() && size.value() > 0.0);
        self.font_size = Some(size);
        self
    }

    #[must_use]
    pub const fn font_family(&self) -> Option<&SharedString> {
        self.font_family.as_ref()
    }

    #[must_use]
    pub const fn font_size(&self) -> Option<Pixels> {
        self.font_size
    }

    /// Resolves each missing field independently from a lower-priority typography source.
    #[must_use]
    pub fn with_fallback(&self, fallback: &Self) -> Self {
        Self {
            font_family: self
                .font_family
                .clone()
                .or_else(|| fallback.font_family.clone()),
            font_size: self.font_size.or(fallback.font_size),
        }
    }
}
