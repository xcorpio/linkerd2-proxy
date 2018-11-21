use futures::{future, sync::mpsc, Poll, Stream};
use http::HeaderMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tower_grpc::{self as grpc, Response};

use api::tap as api;
use convert::*;

use super::Subscribe;

// Buffer ~10 req/rsp pairs' worth of events.
const PER_REQUEST_BUFFER_CAPACITY: usize = 40;

#[derive(Debug)]
pub struct Server<T: Subscribe<Tap>> {
    subscribe: T,
}

pub struct ResponseStream {
    rx: mpsc::Receiver<api::TapEvent>,
    tap: Arc<Tap>,
}

pub struct Tap {
    tx: Option<mpsc::Sender<api::TapEvent>>,
    match_: Match,
    remaining: AtomicUsize,
}

impl<T: Subscribe<Tap>> Server<T> {
    pub(super) fn new(subscribe: T) -> Self {
        Self { subscribe }
    }
}

fn invalid_arg<V: http::header::HeaderValue>(msg: V) -> grpc::Error {
    let status = grpc::Status::with_code(grpc::Code::InvalidArgument);
    let mut headers = HeaderMap::new();
    headers.insert("grpc-message", msg);
    grpc::Error::Grpc(status, headers)
}

impl<T: Subscribe<Tap>> api::server::Tap for Server<T> {
    type ObserveStream = ResponseStream;
    type ObserveFuture = future::FutureResult<Response<Self::ObserveStream>, grpc::Error>;

    fn observe(&mut self, req: grpc::Request<ObserveRequest>) -> Self::ObserveFuture {
        let req = req.into_inner();

        if req.limit == 0 {
            return future::err(invalid_arg("limit must be positive"));
        }

        let match_ = match Match::new(&req.match_match_) {
            Ok(m) => m,
            Err(e) => {
                return future::err(invalid_arg(format("{}", e)));
            }
        };

        let (tx, rx) = mpsc::channel(PER_REQUEST_BUFFER_CAPACITY);
        let tap = Arc::new(Tap {
            match_,
            tx: Some(tx),
            remaining: AtomicUsize::new(req.limit as usize),
        });
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
