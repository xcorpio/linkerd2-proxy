use http;
use futures::sync::oneshot;
use futures::{Future, Stream};
use futures_mpsc_lossy;
use indexmap::IndexMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Weak,
};

use api::tap as api;
use svc;

pub mod event;
mod grpc;
mod match_;
mod pb;
mod service;

pub use self::grpc::Server;

trait Register {
    type Tap: Tap;
    type Taps: Stream<Item = Weak<Self::Tap>>;

    fn register(&mut self) -> Self::Taps;
}

trait Subscribe<T: Tap> {
    fn subscribe(&mut self, tap: Weak<T>);
}

trait Tap {
    type TapRequestBody: TapBody;
    type TapResponse: TapResponse<TapBody = Self::TapResponseBody>;
    type TapResponseBody: TapBody;

    fn tap<B: Payload>(&self, req: &http::Request<B>)
        -> Option<(Self::TapRequestBody, Self::TapResponse)>;
}

trait TapBody {
    fn data<D: IntoBuf>(&mut self, data: &D::Buf);

    fn end(self, headers: Option<&http::HeaderMap>);

    fn fail(self, error: &h2::Error);
}

trait TapResponse {
    type TapBody: TapBody;

    fn tap<B: Payload>(self, rsp: &http::Response<B>) -> Self::TapBody;

    fn fail<E: HasH2Reason>(self, error: &E);
}
