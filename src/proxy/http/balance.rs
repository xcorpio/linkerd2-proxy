extern crate tower_balance;
extern crate tower_discover;
extern crate tower_h2_balance;

use futures::{Async, Poll, Stream};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::time::Duration;

pub use self::tower_balance::{choose::PowerOfTwoChoices, load::WithPeakEwma, Balance};
use self::tower_discover::{Change, Discover as TowerDiscover};
pub use self::tower_h2_balance::{PendingUntilFirstData, PendingUntilFirstDataBody};

use svc;

#[derive(Debug, Clone)]
pub enum Update<Endpoint> {
    Make(SocketAddr, Endpoint),
    Remove(SocketAddr),
}

trait Resolve<Target> {
    type Endpoint;
    type Updates: Stream<Item = Update<Self::Endpoint>>;

    fn resolve(&self, target: &Target) -> Self::Updates;
}

pub struct Layer<T, R: Resolve<T> + Clone>  {
    resolve: R,
    _p: PhantomData<fn() -> T>,
}

pub struct Make<T, R: Resolve<T>, M: svc::Make<R::Endpoint>> {
    decay: Duration,
    resolve: R,
    make_endpoint: M,
    _p: PhantomData<fn() -> T>,
}

struct Discover<E, U: Stream<Item = Update<E>>, M: svc::Make<E>> {
    updates: U,
    make_endpoint: M,
    _p: PhantomData<fn() -> E>,
}

impl<T, R, M> svc::Make<T> for Make<T, R, M>
where
    R: Resolve<T>,
    M: svc::Make<R::Endpoint>,
{
    type Output = Balance<
        WithPeakEwma<Discover<R::Endpoint, R::Updates, M>, PendingUntilFirstData>,
        PowerOfTwoChoices,
    >;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
        let discover = Discover {
            updates: self.resolve.resolve(&target),
            make_endpoint: self.make_endpoint.clone(),
        };

        let instrument = PendingUntilFirstData::default();
        let loaded = WithPeakEwma::new(discover, self.decay, instrument);
        Balance::p2c(loaded)
    }
}

impl<E, U, M> TowerDiscover for Discover<E, U, M>
where
{
    type Key = SocketAddr;
    type Request = <M::Output as svc::Service>::Request;
    type Response = <M::Output as svc::Service>::Response;
    type Error = <M::Output as svc::Service>::Error;
    type Service = M::Output;
    type DiscoverError = M::Error;

    fn poll(&mut self)
        -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError>
    {
        loop {
            let up = try_ready!(self.updates.poll().map(Error::Update))?;
            trace!("watch: {:?}", up);
            match up {
                Update::Make(addr, endpoint) => {
                    // We expect the load balancer to handle duplicate inserts
                    // by replacing the old endpoint with the new one, so
                    // insertions of new endpoints and metadata changes for
                    // existing ones can be handled in the same way.
                    let svc = self.new_endpoint.make(&endpoint).map_err(Error::Make)?;
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
pub enum Error<U, M> {
    Update(U),
    Make(M),
}
