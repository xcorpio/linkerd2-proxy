extern crate tower_balance;
extern crate tower_discover;
extern crate tower_h2_balance;

use futures::Stream;
use std::net::SocketAddr;

pub use self::tower_balance::{choose, load::WithPeakEwma, Balance};
pub use self::tower_h2_balance::{PendingUntilFirstData, PendingUntilFirstDataBody};
pub use self::tower_discover::Discover;

use svc;

#[derive(Debug, Clone)]
enum Update<Meta> {
    Make(SocketAddr, Meta),
    Remove(SocketAddr),
}

pub struct Layer<U: Stream<Item = Update, Error = ()>>  {
    updates: U,
}

pub struct Make {

}

impl<T, E, M: svc::Make<E>> svc::Make<T> for Make
{
    type Output = Balance<
        WithPeakEwma<D, PendingUntilFirstData>,
        choose::PowerOfTwoChoices,
    >;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
        let instrument = PendingUntilFirstData::default();
        let loaded = load::WithPeakEwma::new(resolve, DEFAULT_DECAY, instrument);
        Balance::p2c(loaded)
    }
}
