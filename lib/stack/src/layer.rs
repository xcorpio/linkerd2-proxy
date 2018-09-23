/// A stackable element.
///
/// Given a `Next`-typed inner value, produces a `Make`-typed value.
/// This is especially useful for composable types like `Make`s.
pub trait Layer<Target, NextTarget, Next: super::Make<NextTarget>> {
    type Value;
    type Error;
    type Make: super::Make<Target, Value = Self::Value, Error = Self::Error>;

    /// Produce a `Make` value from a `Next` value.
    fn bind(&self, next: Next) -> Self::Make;
}
