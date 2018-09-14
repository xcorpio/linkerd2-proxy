use futures::{future::MapErr, Async, Poll, Stream};
use futures_watch;
use std::marker::PhantomData;

use svc;

#[derive(Clone,Debug)]
pub struct Layer<V> {
    watch: futures_watch::Watch<V>,
}

#[derive(Clone, Debug)]
pub struct Make<V, T, M: svc::MakeClient<(T, V)>> {
    watch: futures_watch::Watch<V>,
    make_client: M,
    target: T,
}

/// A Service that updates itself as a Watch updates.
#[derive(Clone, Debug)]
pub struct Service<EV, T, M: svc::MakeClient<(T, V)>> {
    watch: futures_watch::Watch<V>,
    make_client: M,
    inner: M::Client,
    _p: PhantomData<T>,
}

#[derive(Debug)]
pub enum Error<I, M> {
    Make(M),
    Inner(I),
}

impl<V, T, M> svc::Layer<M> for Layer<V>
where
    T: Clone,
    M: svc::MakeClient<(T, V)>,
    M: Clone,
{
    type Bound = Make<V, T, M::Client>;

    fn bind(&self, make_client: M) -> Self::Bound {
        Make {
            watch: self.watch.clone(),
            make_client,
            _p: PhantomData,
        }
    }
}

impl<V, T, M> svc::MakeClient<T> for Make<V, T, M>
where
    T: Clone,
    M: svc::MakeClient<(T, V)>,
    M: Clone,
{
    type Error = M::Error;
    type Client = Service<V, T, M::Client>;

    fn make_client(&self, target: &T) -> Result<Self::Client, Self::Error> {
        let v = self.watch.borrow();
        let inner = self.make_client.make_client(&(target, *v))?;
        Make {
            watch: self.watch.clone(),
            make_client: self.make_client.clone(),
            target: target.clone(),
            _p: PhantomData,
        }
    }
}

impl<V, T, M> svc::Service for Service<V, T, M>
where
    T: Clone,
    V: Clone,
    M: svc::MakeClient<(T, V)>,
{
    type Request = <M::Client as Service>::Request;
    type Response = <M::Client as Service>::Response;
    type Error = Error<<M::Client as Service>::Error, M::Error>;
    type Future = MapErr<
        <M::Client as Service>::Future,
        fn(<M::Client as Service>::Error) -> M::Error,
    >;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        // Check to see if the watch has been updated and, if so, rebind the service.
        //
        // `watch.poll()` can't actually fail; so errors are not considered.
        while let Ok(Async::Ready(Some(()))) = self.watch.poll() {
            let target = (self.target, *self.watch.borrow());
            let client = self.make_client.make_client(&target).map_err(Error::Make)?;
            self.inner = client;
        }

        self.inner.poll_ready().map_err(Error::Inner)
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        self.inner.call(req).map_err(Error::Inner)
    }
}

#[cfg(test)]
mod tests {
    use futures::future;
    use std::time::Duration;
    use task::test_util::BlockOnFor;
    use tokio::runtime::current_thread::Runtime;
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(60);

    #[test]
    fn rebind() {
        struct Svc(usize);
        impl Service for Svc {
            type Request = ();
            type Response = usize;
            type Error = ();
            type Future = future::FutureResult<usize, ()>;
            fn poll_ready(&mut self) -> Poll<(), Self::Error> {
                Ok(().into())
            }
            fn call(&mut self, _: ()) -> Self::Future {
                future::ok(self.0)
            }
        }

        let mut rt = Runtime::new().unwrap();
        macro_rules! assert_ready {
            ($svc:expr) => {
                rt.block_on_for(TIMEOUT, future::poll_fn(|| $svc.poll_ready()))
                    .expect("ready")
            };
        }
        macro_rules! call {
            ($svc:expr) => {
                rt.block_on_for(TIMEOUT, $svc.call(()))
                    .expect("call")
            };
        }

        let (watch, mut store) = futures_watch::Watch::new(1);
        let mut svc = Watch::new(watch, |n: &usize| Svc(*n));

        assert_ready!(svc);
        assert_eq!(call!(svc), 1);

        assert_ready!(svc);
        assert_eq!(call!(svc), 1);

        store.store(2).expect("store");
        assert_ready!(svc);
        assert_eq!(call!(svc), 2);

        store.store(3).expect("store");
        store.store(4).expect("store");
        assert_ready!(svc);
        assert_eq!(call!(svc), 4);

        drop(store);
        assert_ready!(svc);
        assert_eq!(call!(svc), 4);
    }
}
