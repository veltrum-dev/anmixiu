use std::{borrow::Borrow, fmt, ops::Deref, sync::Arc};

use smol_str::{SmolStr, SmolStrBuilder};

/// An immutable UI string optimized for cheap cloning and short values.
///
/// Values up to 23 bytes are stored inline, long dynamic values use shared heap storage, and
/// [`new_static`](Self::new_static) borrows a static string without allocation or copying.
#[repr(transparent)]
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SharedString(SmolStr);

impl SharedString {
    #[must_use]
    pub const fn new_static(value: &'static str) -> Self {
        Self(SmolStr::new_static(value))
    }

    #[must_use]
    pub fn new(value: impl AsRef<str>) -> Self {
        Self(SmolStr::new(value))
    }

    #[must_use]
    /// Formats directly into short-string storage.
    ///
    /// # Panics
    ///
    /// Panics only if `SmolStrBuilder`'s currently infallible formatting implementation reports
    /// an error.
    pub fn from_format(arguments: fmt::Arguments<'_>) -> Self {
        let mut builder = SmolStrBuilder::new();
        fmt::write(&mut builder, arguments).expect("writing into SmolStrBuilder is infallible");
        Self(builder.finish())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub const fn is_heap_allocated(&self) -> bool {
        self.0.is_heap_allocated()
    }
}

impl Default for SharedString {
    fn default() -> Self {
        Self::new_static("")
    }
}

impl Deref for SharedString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for SharedString {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for SharedString {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for SharedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for SharedString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&SharedString> for SharedString {
    fn from(value: &SharedString) -> Self {
        value.clone()
    }
}

impl From<&str> for SharedString {
    fn from(value: &str) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<String> for SharedString {
    fn from(value: String) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<Arc<str>> for SharedString {
    fn from(value: Arc<str>) -> Self {
        Self(SmolStr::from(value))
    }
}

impl From<SharedString> for String {
    fn from(value: SharedString) -> Self {
        value.0.into()
    }
}

impl From<SharedString> for Arc<str> {
    fn from(value: SharedString) -> Self {
        value.0.into()
    }
}

#[macro_export]
macro_rules! shared_format {
    ($($argument:tt)*) => {
        $crate::SharedString::from_format(format_args!($($argument)*))
    };
}
