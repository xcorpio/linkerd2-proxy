use bytes::IntoBuf;
use futures::Stream;
use http;
use indexmap::IndexMap;
use std::net;
use std::sync::Weak;
use tower_h2::Body as Payload;

use proxy::http::HasH2Reason;

mod daemon;
mod grpc;
mod service;

pub type Layer<I> = service::Layer<daemon::Register<grpc::Tap<I>>>;
pub type Server<I> = grpc::Server<I, daemon::Subscribe<grpc::Tap<I>>>;
pub type Daemon<I> = daemon::Daemon<grpc::Tap<I>>;

pub fn new<I: Inspect + Clone>(inspect: I) -> (Layer<I>, Server<I>, Daemon<I>) {
    let (daemon, register, subscribe) = daemon::new();
    let layer = service::layer(register);
    let server = Server::new(inspect, subscribe);
    (layer, server, daemon)
}

pub trait Inspect {
    fn src_addr<B>(&self, req: &http::Request<B>) -> Option<&net::SocketAddr>;
    fn dst_addr<B>(&self, req: &http::Request<B>) -> Option<&net::SocketAddr>;
    fn dst_labels<B>(&self, req: &http::Request<B>) -> Option<&IndexMap<String, String>>;
}

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

    fn tap<B: Payload>(
        &self,
        req: &http::Request<B>,
    ) -> Option<(Self::TapRequestBody, Self::TapResponse)>;
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
