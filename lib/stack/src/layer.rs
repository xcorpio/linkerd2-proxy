use std::marker::PhantomData;

use super::when;

/// A stackable element.
///
/// Given a `Next`-typed inner value, produces a `Stack`-typed value.
/// This is especially useful for composable types like `Stack`s.
pub trait Layer<Target, NextTarget, Next: super::Stack<NextTarget>> {
    type Value;
    type Error;
    type Stack: super::Stack<Target, Value = Self::Value, Error = Self::Error>;

    /// Produce a `Stack` value from a `Next` value.
    fn bind(&self, next: Next) -> Self::Stack;

    /// Compose this `Layer` with another.
    fn and_then<T, N, L>(self, inner: L)
        -> AndThen<Target, NextTarget, T, N, Self, L>
    where
        N: super::Stack<T>,
        L: Layer<NextTarget, T, N>,
        Self: Layer<Target, NextTarget, L::Stack> + Sized,
    {
        AndThen {
            outer: self,
            inner,
            _p: PhantomData,
        }
    }

    fn and_when<P, N, L>(self, predicate: P, inner: L)
        -> AndThen<Target, NextTarget, NextTarget, N, Self, when::Layer<NextTarget, P, N, L>>
    where
        P: when::Predicate<NextTarget> + Clone,
        N: super::Stack<NextTarget> + Clone,
        L: Layer<NextTarget, NextTarget, N, Error = N::Error> + Clone,
        Self: Layer<Target, NextTarget, when::Stack<NextTarget, P, N, L>> + Sized,
    {
        AndThen {
            outer: self,
            inner: when::Layer::new(predicate, inner),
            _p: PhantomData,
        }
    }
}

/// Combines two `Layers` into one layer.
#[derive(Debug, Clone)]
pub struct AndThen<A, B, C, Next, Outer, Inner>
where
    Outer: Layer<A, B, Inner::Stack>,
    Inner: Layer<B, C, Next>,
    Next: super::Stack<C>,
{
    outer: Outer,
    inner: Inner,
    // `AndThen` should be Send/Sync independently of `Next`.
    _p: PhantomData<fn() -> (A, B, C, Next)>,
}

impl<A, B, C, Next, Outer, Inner> Layer<A, C, Next>
    for AndThen<A, B, C, Next, Outer, Inner>
where
    Outer: Layer<A, B, Inner::Stack>,
    Inner: Layer<B, C, Next>,
    Next: super::Stack<C>,
{
    type Value = Outer::Value;
    type Error = Outer::Error;
    type Stack = Outer::Stack;

    fn bind(&self, next: Next) -> Self::Stack {
        self.outer.bind(self.inner.bind(next))
    }
}
