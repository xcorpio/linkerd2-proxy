use std::marker::PhantomData;
use std::time::Duration;

use svc;
pub use timeout::Timeout;

pub struct Layer<T> {
    timeout: Duration,
    _p: PhantomData<fn() -> T>
}

pub struct Make<T, M> {
    inner: M,
    timeout: Duration,
    _p: PhantomData<fn() -> T>
}

impl<T, M> Layer<T>
where
    M: svc::Make<T>,
{
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            _p: PhantomData
        }
    }
}

impl<T, M> svc::Layer<M> for Layer<T>
where
    M: svc::Make<T>,
{
    type Bound = Make<T, M>;

    fn bind(&self, inner: M) -> Self::Bound {
        Make {
            inner,
            timeout: self.timeout,
            _p: PhantomData
        }
    }
}

impl<T, M> svc::Make<T> for Make<T, M>
where
    M: svc::Make<T>,
{
    type Output = Timeout<M::Output>;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
        let inner = self.inner.make(&target)?;
        Ok(Timeout::new(inner, self.timeout))
    }
}
