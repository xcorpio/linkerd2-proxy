extern crate tower_buffer;
extern crate tower_in_flight_limit;

use std::fmt;
use std::marker::PhantomData;

pub use self::tower_buffer::{Buffer, SpawnError};
pub use self::tower_in_flight_limit::InFlightLimit;

use logging;
use svc;

pub struct Layer<T> {
    max_in_flight: usize,
    _p: PhantomData<fn() -> T>
}

pub struct Make<T, M> {
    max_in_flight: usize,
    inner: M,
    _p: PhantomData<fn() -> T>
}

#[derive(Debug)]
pub enum Error<M, S> {
    Make(M),
    Spawn(S),
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

impl<T, M> svc::Layer<M> for Layer<T>
where
    M: svc::Make<T>,
    M::Output: svc::Service,
{
    type Bound = Make<T, M>;

    fn bind(&self, inner: M) -> Self::Bound {
        Make {
            inner,
            max_in_flight: self.max_in_flight,
            _p: PhantomData
        }
    }
}

impl<T, M> svc::Make<T> for Make<T, M>
where
    T: fmt::Display + Clone,
    M: svc::Make<T>,
    M::Output: svc::Service,
{
    type Output = InFlightLimit<Buffer<M::Output>>;
    type Error = Error<M::Error, M::Output>;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
        let inner = self.inner.make(&target).map_err(Error::Make)?;

        let executor = logging::context_executor(target.clone());
        let buffer = Buffer::new(inner, executor).map_err(Error::Spawn)?;

        Ok(InFlightLimit::new(buffer, self.max_in_flight))
    }
}
