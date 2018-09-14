// TODO move to `timeout` crate.

use std::marker::PhantomData;
use std::time::Duration;

use svc;
pub use timeout::Timeout;

#[derive(Debug)]
pub struct Layer<T, M> {
    timeout: Duration,
    _p: PhantomData<fn() -> (T, M)>
}

#[derive(Debug)]
pub struct Make<T, M> {
    inner: M,
    timeout: Duration,
    _p: PhantomData<fn() -> T>
}

impl<T, M> Layer<T, M> {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            _p: PhantomData
        }
    }
}

impl<T, M> Clone for Layer<T, M> {
    fn clone(&self) -> Self {
        Self::new(self.timeout)
    }
}

impl<T, M> svc::Layer<T, T, M> for Layer<T, M>
where
    M: svc::Make<T>,
{
    type Value = <Make<T, M> as svc::Make<T>>::Value;
    type Error = <Make<T, M> as svc::Make<T>>::Error;
    type Make = Make<T, M>;

    fn bind(&self, inner: M) -> Self::Make {
        Make {
            inner,
            timeout: self.timeout,
            _p: PhantomData
        }
    }
}

impl<T, M: Clone> Clone for Make<T, M> {
    fn clone(&self) -> Self {
        Make {
            inner: self.inner.clone(),
            timeout: self.timeout,
            _p: PhantomData,
        }
    }
}

impl<T, M> svc::Make<T> for Make<T, M>
where
    M: svc::Make<T>,
{
    type Value = Timeout<M::Value>;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target)?;
        Ok(Timeout::new(inner, self.timeout))
    }
}
