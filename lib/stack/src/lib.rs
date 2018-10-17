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
}

/// Implements `Stack<T>` for any `T` by cloning a `V`-typed value.
pub mod shared {
    use std::{error, fmt};

    pub fn stack<V: Clone>(v: V) -> Stack<V> {
        Stack(v)
    }

    #[derive(Debug)]
    pub struct Stack<V: Clone>(V);

    #[derive(Debug)]
    pub enum Error {}

    impl<V: Clone> Clone for Stack<V> {
        fn clone(&self) -> Self {
            stack(self.0.clone())
        }
    }

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
