use std::marker::PhantomData;

use super::when;

/// A stackable element.
///
/// Given a `M`-typed inner value, produces a `Stack`-typed value.
/// This is especially useful for composable types like `Stack`s.
pub trait Layer<T, U, M: super::Stack<U>> {
    type Value;
    type Error;
    type Stack: super::Stack<T, Value = Self::Value, Error = Self::Error>;

    /// Produce a `Stack` value from a `M` value.
    fn bind(&self, next: M) -> Self::Stack;

    /// Compose this `Layer` with another.
    fn and_then<V, N, L>(self, inner: L)
        -> AndThen<T, U, V, N, Self, L>
    where
        N: super::Stack<V>,
        L: Layer<U, V, N>,
        Self: Layer<T, U, L::Stack> + Sized,
    {
        AndThen {
            outer: self,
            inner,
            _p: PhantomData,
        }
    }

    fn and_when<P, N, L>(self, predicate: P, inner: L)
        -> AndThen<T, U, U, N, Self, when::Layer<U, P, N, L>>
    where
        P: when::Predicate<U> + Clone,
        N: super::Stack<U> + Clone,
        L: Layer<U, U, N, Error = N::Error> + Clone,
        Self: Layer<T, U, when::Stack<U, P, N, L>> + Sized,
    {
        AndThen {
            outer: self,
            inner: when::Layer::new(predicate, inner),
            _p: PhantomData,
        }
    }
}

/// Combines two `Layers` as one.
///
/// Given an `Outer: Layer<T, U, _>` and an `Inner: Layer<U, V, _>`, producesa
/// `Layer<T, C, _>`, encapsulating the logic of the Outer and Inner layers.
#[derive(Debug, Clone)]
pub struct AndThen<T, U, V, M, Outer, Inner>
where
    Outer: Layer<T, U, Inner::Stack>,
    Inner: Layer<U, V, M>,
    M: super::Stack<V>,
{
    outer: Outer,
    inner: Inner,
    // `AndThen` should be Send/Sync independently of `M`.
    _p: PhantomData<fn() -> (T, U, V, M)>,
}

impl<T, U, V, M, Outer, Inner> Layer<T, V, M>
    for AndThen<T, U, V, M, Outer, Inner>
where
    Outer: Layer<T, U, Inner::Stack>,
    Inner: Layer<U, V, M>,
    M: super::Stack<V>,
{
    type Value = Outer::Value;
    type Error = Outer::Error;
    type Stack = Outer::Stack;

    fn bind(&self, next: M) -> Self::Stack {
        self.outer.bind(self.inner.bind(next))
    }
}
