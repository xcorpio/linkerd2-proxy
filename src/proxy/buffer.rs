extern crate tower_buffer;
extern crate tower_in_flight_limit;

use std::fmt;
use std::marker::PhantomData;

pub use self::tower_buffer::{Buffer, SpawnError};
pub use self::tower_in_flight_limit::InFlightLimit;

use logging;
use svc;

#[derive(Debug)]
pub struct Layer<T> {
    max_in_flight: usize,
    _p: PhantomData<fn() -> T>
}

#[derive(Debug)]
pub struct Make<T, M> {
    max_in_flight: usize,
    inner: M,
    _p: PhantomData<fn() -> T>
}

#[derive(Debug)]
pub enum Error<M, S> {
    Make(M),
    Spawn(SpawnError<S>),
}

impl<T> Default for Layer<T> {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_IN_FLIGHT)
    }
}

impl<T> Layer<T> {
    pub const DEFAULT_MAX_IN_FLIGHT: usize = 10_000;

    fn new(max_in_flight: usize) -> Self {
        Layer {
            max_in_flight,
            _p: PhantomData
        }
    }
}

impl<T, M> svc::Layer<T, T, M> for Layer<T>
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
            max_in_flight: self.max_in_flight,
            _p: PhantomData
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
    type Value = InFlightLimit<Buffer<M::Value>>;
    type Error = Error<M::Error, M::Value>;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target).map_err(Error::Make)?;

        let executor = logging::context_executor(target.clone());
        let buffer = Buffer::new(inner, &executor).map_err(Error::Spawn)?;

        Ok(InFlightLimit::new(buffer, self.max_in_flight))
    }
}
