use futures::{Async, Future, Poll};
use h2;
use http;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_timer::clock;
use tower_h2;

use super::Taps;
use svc::http::{Classify, ClassifyResponse};
use svc::{MakeService, Service, Stack};

/// A stack module that wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Mod<C>
where
    C: Classify,
{
    taps: Arc<Mutex<Taps>>,
    _p: PhantomData<C>,
}

/// Wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Make<N, C>
where
    N: MakeService,
    C: Classify<Error = <N::Service as Service>::Error>,
{
    taps: Arc<Mutex<Taps>>,
    inner: N,
    _p: PhantomData<C>,
}

/// A middleware that records HTTP taps.
#[derive(Clone, Debug)]
pub struct TapService<S, C>
where
    S: Service,
    C: Classify<Error = S::Error>,
{
    taps: Arc<Mutex<Taps>>,
    inner: S,
    _p: PhantomData<C>,
}

pub struct ResponseFuture<S, C>
where
    S: Service<Error = C::Error>,
    C: ClassifyResponse,
{
    classify: Option<C>,
    taps: Arc<Mutex<Taps>>,
    stream_open_at: Instant,
    inner: S::Future,
}

#[derive(Debug)]
pub struct RequestBody<T> {
    taps: Arc<Mutex<Taps>>,
    inner: T,
}

#[derive(Debug)]
pub struct ResponseBody<T, C: ClassifyResponse> {
    classify: Option<C>,

    taps: Arc<Mutex<Taps>>,

    stream_open_at: Instant,
    first_byte_at: Option<Instant>,

    inner: T,
}

// ==== impl Mod ====

impl<C: Classify> Mod<C> {
    pub(super) fn new(taps: Arc<Mutex<Taps>>) -> Self {
        Self {
            taps,
            _p: PhantomData,
        }
    }
}

impl<N, A, B, C> Stack<N> for Mod<C>
where
    N: MakeService,
    N::Service: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = C::Error,
    >,
    C: Classify,
    C::ClassifyResponse: Debug + Send + Sync + 'static,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = <Make<N, C> as MakeService>::Service;
    type MakeService = Make<N, C>;

    fn build(&self, inner: N) -> Self::MakeService {
        Make {
            taps: self.taps.clone(),
            inner,
            _p: PhantomData,
        }
    }
}

// ==== impl Make ====

impl<N, A, B, C> MakeService for Make<N, C>
where
    N: MakeService,
    N::Service: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = C::Error,
    >,
    C: Classify,
    C::ClassifyResponse: Debug + Send + Sync + 'static,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = TapService<N::Service, C>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(config)?;
        Ok(TapService {
            taps: self.taps.clone(),
            inner,
            _p: PhantomData,
        })
    }
}

// === TapService ===

impl<S, A, B, C> Service for TapService<S, C>
where
    S: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = C::Error,
    >,
    C: Classify,
    C::ClassifyResponse: Debug + Send + Sync + 'static,
{
    type Request = http::Request<A>;
    type Response = http::Response<ResponseBody<B, C::ClassifyResponse>>;
    type Error = S::Error;
    type Future = ResponseFuture<S, C::ClassifyResponse>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let req = {
            let (head, inner) = req.into_parts();
            let body = RequestBody {
                taps: self.taps.clone(),
                inner,
            };
            http::Request::from_parts(head, body)
        };

        ResponseFuture {
            classify: req.extensions().get::<C::ClassifyResponse>().cloned(),
            taps: self.taps.clone(),
            stream_open_at: clock::now(),
            inner: self.inner.call(req),
        }
    }
}

impl<S, B, C> Future for ResponseFuture<S, C>
where
    S: Service<Response = http::Response<B>, Error = C::Error>,
    C: ClassifyResponse,
{
    type Item = http::Response<ResponseBody<B, C>>;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (head, inner) = try_ready!(self.inner.poll()).into_parts();

        let body = ResponseBody {
            classify: self.classify.take(),
            taps: self.taps.clone(),
            stream_open_at: self.stream_open_at,
            first_byte_at: None,
            inner,
        };

        let rsp = http::Response::from_parts(head, body);
        Ok(rsp.into())
    }
}

impl<B: tower_h2::Body> tower_h2::Body for RequestBody<B> {
    type Data = B::Data;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let frame = try_ready!(self.inner.poll_data());

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        self.inner.poll_trailers()
    }
}

impl<B, C> ResponseBody<B, C>
where
    C: ClassifyResponse,
{
    fn tap_eos(&mut self, _eos: Option<C::Class>) {}

    fn tap_err(&mut self, err: C::Error) -> C::Error {
        let eos = self.classify.take().map(|mut c| c.error(&err));
        self.tap_eos(eos);
        err
    }
}

impl<B, C> tower_h2::Body for ResponseBody<B, C>
where
    B: tower_h2::Body,
    C: ClassifyResponse<Error = h2::Error>,
{
    type Data = B::Data;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let frame = try_ready!(self.inner.poll_data().map_err(|e| self.tap_err(e)));

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        self.inner.poll_trailers()
    }
}
