extern crate tower_buffer;

use std::fmt;
use std::marker::PhantomData;

pub use self::tower_buffer::{Buffer, SpawnError};

use logging;
use svc;

#[derive(Debug)]
pub struct Layer<T, M>(PhantomData<fn() -> (T, M)>);

#[derive(Debug)]
pub struct Make<T, M> {
    inner: M,
    _p: PhantomData<fn() -> T>
}

pub enum Error<M, S> {
    Make(M),
    Spawn(SpawnError<S>),
}

impl<T, M> Layer<T, M> {
    pub fn new() -> Self {
        Layer(PhantomData)
    }
}

impl<T, M> Clone for Layer<T, M> {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl<T, M> svc::Layer<T, T, M> for Layer<T, M>
where
    T: fmt::Display + Clone + Send + Sync + 'static,
    M: svc::Make<T>,
    M::Value: svc::Service + Send + 'static,
    <M::Value as svc::Service>::Request: Send,
    <M::Value as svc::Service>::Future: Send,
{
    type Value = <Make<T, M> as svc::Make<T>>::Value;
    type Error = <Make<T, M> as svc::Make<T>>::Error;
    type Make = Make<T, M>;

    fn bind(&self, inner: M) -> Self::Make {
        Make {
            inner,
            _p: PhantomData
        }
    }
}

impl<T, M: Clone> Clone for Make<T, M> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            _p: PhantomData,
        }
    }
}

impl<T, M> svc::Make<T> for Make<T, M>
where
    T: fmt::Display + Clone + Send + Sync + 'static,
    M: svc::Make<T>,
    M::Value: svc::Service + Send + 'static,
    <M::Value as svc::Service>::Request: Send,
    <M::Value as svc::Service>::Future: Send,
{
    type Value = Buffer<M::Value>;
    type Error = Error<M::Error, M::Value>;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target).map_err(Error::Make)?;
        let executor = logging::context_executor(target.clone());
        Buffer::new(inner, &executor).map_err(Error::Spawn)
    }
}

impl<M: fmt::Debug, S> fmt::Debug for Error<M, S> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Make(e) => fmt.debug_tuple("buffer::Error::Make").field(e).finish(),
            Error::Spawn(_) => fmt.debug_tuple("buffer::Error::Spawn").finish(),
        }
    }
}

