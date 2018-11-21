use futures::{future, sync::mpsc, Poll, Stream};
use http::HeaderMap;
use indexmap::IndexMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower_grpc::{self as grpc, Response};

use api::tap as api;
use convert::*;

use super::Subscribe;

// Buffer ~10 req/rsp pairs' worth of events.
const PER_REQUEST_BUFFER_CAPACITY: usize = 40;

#[derive(Debug)]
pub struct Server<T: Subscribe> {
    subscribe: T,
}

pub struct ResponseStream {
    rx: mpsc::Receiver<api::TapEvent>,
    tap: Arc<Tap>,
}

struct Tap {
    tx: mpsc::Sender<api::TapEvent>,
    match_: Match,
    remaining: AtomicUsize,
}

impl<T: Subscribe> Server<T> {
    pub(super) fn new(subscribe: T) -> Self {
        Self { subscribe }
    }
}

impl<T: Subscribe> api::server::Tap for Server<T> {
    type ObserveStream = ResponseStream;
    type ObserveFuture = future::FutureResult<Response<Self::ObserveStream>, grpc::Error>;

    fn observe(&mut self, req: grpc::Request<ObserveRequest>) -> Self::ObserveFuture {
        let req = req.into_inner();

        let tap = match Match::new(&req.match_match_) {
            Ok(m) => {
                let remaining = AtomicUsize::new(req.limit as usize);
                let (tx, rx) = mpsc::channel(PER_REQUEST_BUFFER_CAPACITY);
                Arc::new(Tap { tx, match_, remaining })
            }
            Err(_) => {
                let status = grpc::Status::with_code(grpc::Code::InvalidArgument);
                let mut headers = HeaderMap::new();
                headers.insert("grpc-message", format("{}", e));
                return future::err(grpc::Error::Grpc(status, headers));
            }
        };

        self.subscribe.subscribe(Arc::downgrade(&tap));

        future::ok(Response::new(ResponseStream { rx, tap }))
    }
}

impl Stream for ResponseStream {
    type Item = TapEvent;
    type Error = grpc::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        self.rx.poll().or_else(|_| Ok(None.into()))
    }
}
