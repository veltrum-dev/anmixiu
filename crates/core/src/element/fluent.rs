use super::IntoElement;

/// Fluent conditional transformations for element builders.
pub trait FluentBuilder: IntoElement + Sized {
    fn map<U>(self, transform: impl FnOnce(Self) -> U) -> U {
        transform(self)
    }

    #[must_use]
    fn when(self, condition: bool, then: impl FnOnce(Self) -> Self) -> Self {
        if condition { then(self) } else { self }
    }

    #[must_use]
    fn when_else(
        self,
        condition: bool,
        then: impl FnOnce(Self) -> Self,
        otherwise: impl FnOnce(Self) -> Self,
    ) -> Self {
        if condition {
            then(self)
        } else {
            otherwise(self)
        }
    }

    #[must_use]
    fn when_some<T>(self, option: Option<T>, then: impl FnOnce(Self, T) -> Self) -> Self {
        if let Some(value) = option {
            then(self, value)
        } else {
            self
        }
    }

    #[must_use]
    fn when_none<T>(self, option: &Option<T>, then: impl FnOnce(Self) -> Self) -> Self {
        if option.is_none() { then(self) } else { self }
    }
}

impl<T: IntoElement> FluentBuilder for T {}
