use std::{fmt, sync::Arc};

use crate::SharedString;

/// A caller-provided semantic identity that remains stable across renders.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ElementId {
    Name(SharedString),
    Integer(u64),
    NamedInteger(SharedString, u64),
}

impl ElementId {
    #[must_use]
    pub fn named_u64(name: impl Into<SharedString>, integer: u64) -> Self {
        Self::NamedInteger(name.into(), integer)
    }
}

impl From<&str> for ElementId {
    fn from(value: &str) -> Self {
        Self::Name(value.into())
    }
}

impl From<String> for ElementId {
    fn from(value: String) -> Self {
        Self::Name(value.into())
    }
}

impl From<Arc<str>> for ElementId {
    fn from(value: Arc<str>) -> Self {
        Self::Name(value.into())
    }
}

impl From<SharedString> for ElementId {
    fn from(value: SharedString) -> Self {
        Self::Name(value)
    }
}

impl From<u64> for ElementId {
    fn from(value: u64) -> Self {
        Self::Integer(value)
    }
}

impl From<usize> for ElementId {
    fn from(value: usize) -> Self {
        Self::Integer(u64::try_from(value).unwrap_or(u64::MAX))
    }
}

impl<N> From<(N, u64)> for ElementId
where
    N: Into<SharedString>,
{
    fn from((name, integer): (N, u64)) -> Self {
        Self::NamedInteger(name.into(), integer)
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => formatter.write_str(name),
            Self::Integer(integer) => integer.fmt(formatter),
            Self::NamedInteger(name, integer) => write!(formatter, "{name}:{integer}"),
        }
    }
}

/// An ancestor-qualified element identity used as a cross-frame runtime key.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct GlobalElementId(Arc<[ElementId]>);

impl GlobalElementId {
    #[must_use]
    pub fn new(path: impl IntoIterator<Item = ElementId>) -> Self {
        Self(path.into_iter().collect())
    }

    #[must_use]
    pub fn path(&self) -> &[ElementId] {
        &self.0
    }
}

impl fmt::Display for GlobalElementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, id) in self.0.iter().enumerate() {
            if index != 0 {
                formatter.write_str("/")?;
            }
            id.fmt(formatter)?;
        }
        Ok(())
    }
}
