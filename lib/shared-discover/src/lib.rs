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
    D::Key: Clone,
    D::Service: Clone,
{
    let (notify_tx, notify_rx) = mpsc::unbounded();
    let share = Share { notify_tx };
    let bg = Background {
        discover,
        notify_rx: Some(notify_rx),
        notifiers: VecDeque::new(),
        cache: IndexMap::new(),
    };
    (share, bg)
}

pub struct Share<D: Discover> {
    notify_tx: mpsc::UnboundedSender<Notify<D>>,
}

pub struct SharedDiscover<D: Discover> {
    rx: mpsc::UnboundedReceiver<Change<D::Key, D::Service>>,
}

pub struct Background<D: Discover> {
    discover: D,
    notify_rx: Option<mpsc::UnboundedReceiver<Notify<D>>>,
    notifiers: VecDeque<Notify<D>>,
    cache: IndexMap<D::Key, D::Service>,
}

struct Notify<D: Discover> {
    tx: mpsc::UnboundedSender<Change<D::Key, D::Service>>,
}

impl<D: Discover> Clone for Share<D> {
    fn clone(&self) -> Self {
        Self {
            notify_tx: self.notify_tx.clone(),
        }
    }
}

impl<D> Share<D>
where
    D: Discover,
    D::Key: Clone,
    D::Service: Clone,
{
    pub fn share(&self) -> SharedDiscover<D> {
        let (tx, rx) = mpsc::unbounded();
        let _ = self.notify_tx.unbounded_send(Notify { tx });
        SharedDiscover { rx }
    }
}

impl<D: Discover> Discover for SharedDiscover<D>
where
    D: Discover,
    D::Service: Clone,
{
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

impl<D> Background<D>
where
    D: Discover,
    D::Key: Clone,
    D::Service: Clone,
{
    fn update_from_cache(&self, tx: &Notify<D>) -> Result<(), ()> {
        for (key, svc) in self.cache.iter() {
            tx.tx
                .unbounded_send(Change::Insert(key.clone(), svc.clone()))
                .map_err(|_| {})?;
        }

        Ok(())
    }

    fn notify_notifiers(&mut self, change: &Change<D::Key, D::Service>) {
        for _ in 0..self.notifiers.len() {
            let tx = self.notifiers.pop_front().unwrap();
            let c = match change {
                Change::Insert(ref k, ref s) => Change::Insert(k.clone(), s.clone()),
                Change::Remove(ref k) => Change::Remove(k.clone()),
            };
            if tx.tx.unbounded_send(c).is_ok() {
                self.notifiers.push_back(tx);
            }
        }
    }

    fn poll_notify_rx(&mut self) {
        loop {
            match self
                .notify_rx
                .as_mut()
                .map(|ref mut notify_rx| notify_rx.poll())
            {
                Some(Ok(Async::NotReady)) => return,
                None | Some(Err(_)) | Some(Ok(Async::Ready(None))) => {
                    self.notify_rx = None;
                    return;
                }
                Some(Ok(Async::Ready(Some(tx)))) => {
                    if self.update_from_cache(&tx).is_ok() {
                        self.notifiers.push_back(tx);
                    }
                }
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
        loop {
            self.poll_notify_rx();

            if self.notify_rx.is_none() && self.notifiers.is_empty() {
                return Ok(Async::Ready(()));
            }

            let change = try_ready!(self.discover.poll());
            self.notify_notifiers(&change);
            match change {
                Change::Insert(key, svc) => {
                    self.cache.insert(key, svc);
                }
                Change::Remove(key) => {
                    self.cache.remove(&key);
                }
            }
        }
    }
}
