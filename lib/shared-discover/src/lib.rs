#[macro_use]
extern crate futures;
extern crate indexmap;
extern crate tokio;
extern crate tower_discover;
extern crate tower_service as svc;

use futures::{sync::mpsc, Async, Future, Poll, Stream};
use indexmap::IndexMap;
use std::collections::VecDeque;
use tower_discover::{Change, Discover};

pub fn new<D>(discover: D) -> (Share<D>, Background<D>)
where
    D: Discover,
    D::Service: Clone,
{
    let (txtx, txrx) = mpsc::unbounded();
    let bg = Background {
        discover,
        txrx,
        txs: VecDeque::new(),
        cache: IndexMap::new(),
    };
    (Share { txtx }, bg)
}

pub struct Share<D: Discover> {
    txtx: mpsc::UnboundedSender<Tx<D>>,
}

pub struct SharedDiscover<D: Discover> {
    rx: mpsc::UnboundedReceiver<Change<D::Key, D::Service>>,
}

pub struct Background<D: Discover> {
    discover: D,
    txrx: mpsc::UnboundedReceiver<Tx<D>>,
    txs: VecDeque<Tx<D>>,
    cache: IndexMap<D::Key, D::Service>,
}

struct Tx<D: Discover> {
    tx: mpsc::UnboundedSender<Change<D::Key, D::Service>>,
}

impl<D> Share<D>
where
    D: Discover,
    D::Key: Clone,
    D::Service: Clone,
{
    pub fn share(&self) -> SharedDiscover<D> {
        let (tx, rx) = mpsc::unbounded();
        let _ = self.txtx.unbounded_send(Tx { tx });
        SharedDiscover { rx }
    }
}

impl<D> Background<D>
where
    D: Discover,
    D::Key: Clone,
    D::Service: Clone,
{
    fn update_from_cache(&self, tx: &Tx<D>) -> Result<(), ()> {
        for (key, svc) in self.cache.iter() {
            tx.tx
                .unbounded_send(Change::Insert(key.clone(), svc.clone()))
                .map_err(|_| {})?;
        }

        Ok(())
    }

    fn notify_txs(&mut self, change: &Change<D::Key, D::Service>) {
        for _ in 0..self.txs.len() {
            let tx = self.txs.pop_front().unwrap();
            let c = match change {
                Change::Insert(ref k, ref s) => Change::Insert(k.clone(), s.clone()),
                Change::Remove(ref k) => Change::Remove(k.clone()),
            };
            if tx.tx.unbounded_send(c).is_ok() {
                self.txs.push_back(tx);
            }
        }
    }

    fn poll_txrx(&mut self) {
        loop {
            match self.txrx.poll() {
                Ok(Async::NotReady) => return,
                Ok(Async::Ready(Some(tx))) => {
                    if self.update_from_cache(&tx).is_ok() {
                        self.txs.push_back(tx);
                    }
                }
                // No more
                Ok(Async::Ready(None)) | Err(_) => return,
            }
        }
    }
}

impl<D> Future for Background<D>
where
    D: Discover,
    D::Key: Clone,
    D::Service: Clone,
{
    type Item = ();
    type Error = D::DiscoverError;

    fn poll(&mut self) -> Poll<(), Self::Error> {
        self.poll_txrx();

        loop {
            let change = try_ready!(self.discover.poll());
            match change {
                Change::Insert(ref key, ref svc) => {
                    self.cache.insert(key.clone(), svc.clone());
                }
                Change::Remove(ref key) => {
                    self.cache.remove(key);
                }
            }
            self.notify_txs(&change);
        }
    }
}

impl<D: Discover> Discover for SharedDiscover<D> {
    type Key = D::Key;
    type Request = D::Request;
    type Response = D::Response;
    type Error = D::Error;
    type Service = D::Service;
    type DiscoverError = D::DiscoverError;

    fn poll(&mut self) -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError> {
        match self.rx.poll() {
            Ok(Async::Ready(Some(c))) => Ok(Async::Ready(c)),
            Ok(Async::Ready(None)) => Ok(Async::NotReady),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(_) => Ok(Async::NotReady),
        }
    }
}
