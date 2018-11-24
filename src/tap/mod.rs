use http;
use indexmap::IndexMap;
use std::net;

mod daemon;
mod grpc;
mod service;

pub type Layer = service::Layer<daemon::Register<grpc::Tap>>;
pub type Server = grpc::Server<daemon::Subscribe<grpc::Tap>>;
pub type Daemon = daemon::Daemon<grpc::Tap>;

pub fn new() -> (Layer, Server, Daemon) {
    let (daemon, register, subscribe) = daemon::new();
    let layer = Layer::new(register);
    let server = Server::new(subscribe);
    (layer, server, daemon)
}

pub trait Inspect {
    fn src_addr<B>(&self, req: &http::Request<B>) -> Option<net::SocketAddr>;
    //fn src_labels<B>(&self, req: &http::Request<B>) -> Option<&IndexMap<String, String>>;

    fn dst_addr<B>(&self, req: &http::Request<B>) -> Option<net::SocketAddr>;
    fn dst_labels<B>(&self, req: &http::Request<B>) -> Option<&IndexMap<String, String>>;

    fn is_outbound<B>(&self, req: &http::Request<B>) -> bool;

    fn is_inbound<B>(&self, req: &http::Request<B>) -> bool {
        !self.is_outbound(req)
    }
}

mod iface {
    use bytes::IntoBuf;
    use http;
    use futures::Stream;
    use std::sync::Weak;
    use tower_h2::Body as Payload;

    use proxy::http::HasH2Reason;

    pub trait Register {
        type Tap: Tap;
        type Taps: Stream<Item = Weak<Self::Tap>>;

        fn register(&mut self) -> Self::Taps;
    }

    pub trait Subscribe<T: Tap> {
        fn subscribe(&mut self, tap: Weak<T>);
    }

    pub trait Tap {
        type TapRequestBody: TapBody;
        type TapResponse: TapResponse<TapBody = Self::TapResponseBody>;
        type TapResponseBody: TapBody;

        fn tap<B: Payload, I: super::Inspect>(
            &self,
            req: &http::Request<B>,
            inspect: &I,
        ) -> Option<(Self::TapRequestBody, Self::TapResponse)>;
    }

    pub trait TapBody {
        fn data<D: IntoBuf>(&mut self, data: &D::Buf);

        fn eos(self, headers: Option<&http::HeaderMap>);

        fn fail(self, error: &h2::Error);
    }

    pub trait TapResponse {
        type TapBody: TapBody;

        fn tap<B: Payload>(self, rsp: &http::Response<B>) -> Self::TapBody;

        fn fail<E: HasH2Reason>(self, error: &E);
    }
}
