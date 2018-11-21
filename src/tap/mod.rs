use http;
use futures::sync::oneshot;
use futures::{Future, Stream};
use futures_mpsc_lossy;
use indexmap::IndexMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use api::tap as api;
use svc;

pub mod event;
mod grpc;
mod match_;
mod pb;
mod service;

use self::event::{Direction, Endpoint, Event};
pub use self::grpc::Server;
use self::match_;
pub use self::service::layer;

use std::collections::VecDeque;

pub trait Subscribe<M: Match, R: Recv> {
    fn subscribe(&mut self, match_: M, recv: R);
}

pub trait Match {
    type Publish: publish::Init<OpenRequest>;
    type OpenRequest: publish::Init;

    fn match_<B>(&self, req: &http::Request<B>) -> Option<Self::Publish>;
}

pub struct Daemon<M: Match> {
    active: VecDeque<Arc<M>>,
    match_rx: futures_mpsc_lossy::Receiver<M>,
    match_request_rx: futures_mpsc_lossy::Receiver<oneshot::Sender<Weak<M>>>,
}
