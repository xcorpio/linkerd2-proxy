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
    fn match_<B>(&self, req: &http::Request<B>) -> bool;
}

pub trait Recv {
    type Publish: publish::Init;
    fn recv(&mut self) -> Option<Self::Publish>;
}

pub struct Daemon<M: Match, R: Recv> {
    subscriptions: VecDeque<Subscription<M, R>>,
    subscribe_requests: futures_mpsc_lossy::Receiver<(M, R)>,
}

struct Subscription<M: Match, R: Recv> {
    match_: Arc<M>,
    recv: Option<R>,
}

pub mod publish {
    pub trait Init {
        type OpenRequest: OpenRequest<
            EndRequest = Self::EndRequest,
            OpenResponse = Self::OpenResponse,
            EndResponse = Self::EndResponse,
        >;
        type EndRequest: EndRequest;
        type OpenResponse: OpenResponse<EndResponse = EndRespones>;
        type EndResponse: EndResponse;

        type Error;
        type Future: Future<Item = Self::OpenRequest, Error = Self::Error>;

        fn init(&mut self) -> Self::Future;
    }

    pub trait OpenRequest {
        type EndRequest: EndRequest;
        type OpenResponse: OpenResponse<EndResponse = Self::EndRespone>;
        type EndResponse: EndResponse;


        fn open<B>(self, req: http::Request<B>) -> (Self::EndRequest, Self::OpenResponse);
    }

    pub trait EndRequest {
        fn fail(self, err: h2::Reason);
        fn end(self);
    }

    pub trait OpenResponse {
        type EndResponse: EndResponse;
        fn open<B>(self, req: http::Response<B>) -> Self::EndResponse;
    }

    pub trait EndResponse {
        fn fail(self, err: h2::Reason);
        fn end(self);
    }
}
