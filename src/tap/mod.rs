use bytes::IntoBuf;
use futures::Stream;
use http;
use std::sync::Weak;
use tower_h2::Body as Payload;

use proxy::http::HasH2Reason;

mod daemon;
mod grpc;
mod service;

pub type Layer = service::Layer<daemon::Register<grpc::Tap>>;
pub type Server = grpc::Server<daemon::Subscribe<grpc::Tap>>;
pub type Daemon = daemon::Daemon<grpc::Tap>;

pub fn new() -> (Layer, Server, Daemon) {
    let (daemon, register, subscribe) = daemon::new();
    let layer = service::layer(register);
    let server = Server::new(subscribe);
    (layer, server, daemon)
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
