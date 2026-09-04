#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WindowId(u64);

impl WindowId {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}
