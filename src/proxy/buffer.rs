extern crate tower_buffer;

use std::{error, fmt};

pub use self::tower_buffer::{Buffer, Error as ServiceError, SpawnError};

use logging;
use svc;

/// Wraps `Service` stacks with a `Buffer`.
#[derive(Debug, Clone)]
pub struct Layer {
    name: &'static str,
}

/// Produces `Service`s wrapped with a `Buffer`
#[derive(Debug, Clone)]
pub struct Stack<M> {
    inner: M,
    name: &'static str,
}

pub enum Error<M, S> {
    Stack(M),
    Spawn(SpawnError<S>),
}

// === impl Layer ===

pub fn layer(name: &'static str) -> Layer {
    Layer { name }
}

impl<T, M> svc::Layer<T, T, M> for Layer
where
    M: svc::Stack<T>,
    M::Value: svc::Service + Send + 'static,
    <M::Value as svc::Service>::Request: Send,
    <M::Value as svc::Service>::Future: Send,
{
    type Value = <Stack<M> as svc::Stack<T>>::Value;
    type Error = <Stack<M> as svc::Stack<T>>::Error;
    type Stack = Stack<M>;

    fn bind(&self, inner: M) -> Self::Stack {
        Stack {
            inner,
            name: self.name.clone(),
        }
    }
}

// === impl Stack ===


impl<T, M> svc::Stack<T> for Stack<M>
where
    M: svc::Stack<T>,
    M::Value: svc::Service + Send + 'static,
    <M::Value as svc::Service>::Request: Send,
    <M::Value as svc::Service>::Future: Send,
{
    type Value = Buffer<M::Value>;
    type Error = Error<M::Error, M::Value>;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target).map_err(Error::Stack)?;
        let executor = logging::context_executor(self.name.clone());
        Buffer::new(inner, &executor).map_err(Error::Spawn)
    }
}

// === impl Error ===

impl<M: fmt::Debug, S> fmt::Debug for Error<M, S> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Stack(e) => fmt.debug_tuple("buffer::Error::Stack").field(e).finish(),
            Error::Spawn(_) => fmt.debug_tuple("buffer::Error::Spawn").finish(),
        }
    }
}

impl<M: fmt::Display, S> fmt::Display for Error<M, S> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Stack(e) => fmt::Display::fmt(e, fmt),
            Error::Spawn(_) => write!(fmt, "Stack built without an executor"),
        }
    }
}

impl<M: error::Error, S> error::Error for Error<M, S> {
    fn cause(&self) -> Option<&error::Error> {
        match self {
            Error::Stack(e) => e.cause(),
            Error::Spawn(_) => None,
        }
    }
}
