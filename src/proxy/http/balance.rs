extern crate tower_balance;
extern crate tower_discover;
extern crate tower_h2_balance;

use futures::{Async, Poll, Stream};
use http;
use std::fmt;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::time::Duration;
use tower_h2::Body;

pub use self::tower_balance::{choose::PowerOfTwoChoices, load::WithPeakEwma, Balance};
use self::tower_discover::{Change, Discover as TowerDiscover};
pub use self::tower_h2_balance::{PendingUntilFirstData, PendingUntilFirstDataBody};

use proxy::resolve::{Resolve, Resolution, Update};
use svc;

pub struct Layer<T, R: Resolve<T> + Clone>  {
    resolve: R,
    _p: PhantomData<fn() -> T>,
}

pub struct Make<T, R: Resolve<T>, M: svc::Make<R::Endpoint>> {
    decay: Duration,
    resolve: R,
    inner: M,
    _p: PhantomData<fn() -> T>,
}

struct Discover<R: Resolution, M: svc::Make<R::Endpoint>> {
    resolution: R,
    make: M,
}

impl<T, R, M, A, B> svc::Make<T> for Make<T, R, M>
where
    R: Resolve<T>,
    R::Endpoint: fmt::Debug,
    M: svc::Make<R::Endpoint> + Clone,
    M::Output: svc::Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
    A: Body,
    B: Body,
{
    type Output = Balance<
        WithPeakEwma<Discover<R::Resolution, M>, PendingUntilFirstData>,
        PowerOfTwoChoices,
    >;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
        let discover = Discover {
            resolution: self.resolve.resolve(&target),
            make: self.inner.clone(),
        };

        let instrument = PendingUntilFirstData::default();
        let loaded = WithPeakEwma::new(discover, self.decay, instrument);
        Ok(Balance::p2c(loaded))
    }
}

impl<R, M> TowerDiscover for Discover<R, M>
where
    R: Resolution,
    R::Endpoint: fmt::Debug,
    M: svc::Make<R::Endpoint>,
{
    type Key = SocketAddr;
    type Request = <M::Output as svc::Service>::Request;
    type Response = <M::Output as svc::Service>::Response;
    type Error = <M::Output as svc::Service>::Error;
    type Service = M::Output;
    type DiscoverError = Error<R::Error, M::Error>;

    fn poll(&mut self)
        -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError>
    {
        loop {
            let up = try_ready!(self.resolution.poll().map_err(Error::Resolve));
            trace!("watch: {:?}", up);
            match up {
                Update::Make(addr, target) => {
                    // We expect the load balancer to handle duplicate inserts
                    // by replacing the old endpoint with the new one, so
                    // insertions of new endpoints and metadata changes for
                    // existing ones can be handled in the same way.
                    let svc = self.make.make(&target).map_err(Error::Make)?;
                    return Ok(Async::Ready(Change::Insert(addr, svc)));
                },
                Update::Remove(addr) => {
                    return Ok(Async::Ready(Change::Remove(addr)));
                },
            }
        }
    }
}

#[derive(Debug)]
pub enum Error<R, M> {
    Resolve(R),
    Make(M),
}
