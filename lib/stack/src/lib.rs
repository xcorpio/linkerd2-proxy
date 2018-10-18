extern crate futures;
#[macro_use]
extern crate log;
extern crate tower_service as svc;

pub mod either;
pub mod layer;
pub mod stack_new_service;
pub mod stack_per_request;
pub mod watch;

pub use self::either::Either;
pub use self::layer::Layer;
pub use self::stack_new_service::StackNewService;

/// A composable builder.
///
/// A `Stack` attempts to build a `Value` given a `T`-typed _target_. An error is
/// returned iff the target cannot be used to produce a value. Otherwise a value
/// is produced.
///
/// The `Layer` trait provides a mechanism to compose stacks that are generic
/// over another, inner `Stack` type.
pub trait Stack<T> {
    type Value;

    /// Indicates that a given `T` could not be used to produce a `Value`.
    type Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error>;

    /// Wraps this `Stack` with an `L`-typed `Layer` to produce a `Stack<U>`.
    fn push<U, L>(self, layer: L) -> L::Stack
    where
        L: Layer<U, T, Self>,
        Self: Sized,
    {
        layer.bind(self)
    }

    fn map_err<M>(self, m: M) -> map_err::Stack<Self, M>
    where
        M: map_err::MapErr<Self::Error>,
        Self: Sized,
    {
        map_err::stack(self, m)
    }
}

pub mod map_err {

    pub fn layer<E, M>(map_err: M) -> Layer<M>
    where
        M: MapErr<E>,
    {
        Layer(map_err)
    }

    pub(super) fn stack<T, S, M>(inner: S, map_err: M) -> Stack<S, M>
    where
        S: super::Stack<T>,
        M: MapErr<S::Error>,
    {
        Stack {
            inner,
            map_err,
        }
    }

    pub trait MapErr<Input> {
        type Output;

        fn map_err(&self, e: Input) -> Self::Output;
    }

    #[derive(Clone, Debug)]
    pub struct Layer<M>(M);

    #[derive(Clone, Debug)]
    pub struct Stack<S, M> {
        inner: S,
        map_err: M,
    }

    impl<T, S, M> super::Layer<T, T, S> for Layer<M>
    where
        S: super::Stack<T>,
        M: MapErr<S::Error> + Clone,
    {
        type Value = <Stack<S, M> as super::Stack<T>>::Value;
        type Error = <Stack<S, M> as super::Stack<T>>::Error;
        type Stack = Stack<S, M>;

        fn bind(&self, inner: S) -> Self::Stack {
            Stack {
                inner,
                map_err: self.0.clone(),
            }
        }
    }

    impl<T, S, M> super::Stack<T> for Stack<S, M>
    where
        S: super::Stack<T>,
        M: MapErr<S::Error>,
    {
        type Value = S::Value;
        type Error = M::Output;

        fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
            self.inner.make(target).map_err(|e| self.map_err.map_err(e))
        }
    }

    impl<F, I, O> MapErr<I> for F
    where
        F: Fn(I) -> O,
    {
        type Output = O;
        fn map_err(&self, i: I) -> O {
            (self)(i)
        }
    }
}

pub mod phantom_data {
    use std::marker::PhantomData;

    pub fn layer<T, M>() -> Layer<T, M>
    where
        M: super::Stack<T>,
    {
        Layer(PhantomData)
    }

    #[derive(Clone, Debug)]
    pub struct Layer<T, M>(PhantomData<fn() -> (T, M)>);

    #[derive(Clone, Debug)]
    pub struct Stack<T, M> {
        inner: M,
        _p: PhantomData<fn() -> (T, M)>,
    }

    impl<T, M: super::Stack<T>> super::Layer<T, T, M> for Layer<T, M> {
        type Value = <Stack<T, M> as super::Stack<T>>::Value;
        type Error = <Stack<T, M> as super::Stack<T>>::Error;
        type Stack = Stack<T, M>;

        fn bind(&self, inner: M) -> Self::Stack {
            Stack {
                inner,
                _p: PhantomData
            }
        }
    }

    impl<T, M: super::Stack<T>> super::Stack<T> for Stack<T, M> {
        type Value = M::Value;
        type Error = M::Error;

        fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
            self.inner.make(target)
        }
    }
}


pub mod map_target {

    pub fn layer<T, M>(map_target: M) -> Layer<M>
    where
        M: MapTarget<T>,
    {
        Layer(map_target)
    }

    pub trait MapTarget<T> {
        type Target;

        fn map_target(&self, t: &T) -> Self::Target;
    }

    #[derive(Clone, Debug)]
    pub struct Layer<M>(M);

    #[derive(Clone, Debug)]
    pub struct Stack<S, M> {
        inner: S,
        map_target: M,
    }

    impl<T, S, M> super::Layer<T, M::Target, S> for Layer<M>
    where
        S: super::Stack<M::Target>,
        M: MapTarget<T> + Clone,
    {
        type Value = <Stack<S, M> as super::Stack<T>>::Value;
        type Error = <Stack<S, M> as super::Stack<T>>::Error;
        type Stack = Stack<S, M>;

        fn bind(&self, inner: S) -> Self::Stack {
            Stack {
                inner,
                map_target: self.0.clone(),
            }
        }
    }

    impl<T, S, M> super::Stack<T> for Stack<S, M>
    where
        S: super::Stack<M::Target>,
        M: MapTarget<T>,
    {
        type Value = S::Value;
        type Error = S::Error;

        fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
            self.inner.make(&self.map_target.map_target(target))
        }
    }

    impl<F, T, U> MapTarget<T> for F
    where
        F: Fn(&T) -> U,
    {
        type Target = U;
        fn map_target(&self, t: &T) -> U {
            (self)(t)
        }
    }
}

/// Implements `Stack<T>` for any `T` by cloning a `V`-typed value.
pub mod shared {
    use std::{error, fmt};

    pub fn stack<V: Clone>(v: V) -> Stack<V> {
        Stack(v)
    }

    #[derive(Clone, Debug)]
    pub struct Stack<V: Clone>(V);

    #[derive(Debug)]
    pub enum Error {}

    impl<T, V: Clone> super::Stack<T> for Stack<V> {
        type Value = V;
        type Error = Error;

        fn make(&self, _: &T) -> Result<V, Error> {
            Ok(self.0.clone())
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, _: &mut fmt::Formatter) -> fmt::Result {
            unreachable!()
        }
    }

    impl error::Error for Error {}
}
