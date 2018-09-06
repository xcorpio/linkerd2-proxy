use buf::{Buf, IntoBuf};
use futures::{Async, Future, Poll};
use h2;
use http;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_timer::clock;
use tower_h2;

use super::{ctx, Taps};
use svc::{MakeService, Service, Stack};

/// A stack module that wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Mod {
    taps: Arc<Mutex<Taps>>,
}

/// Wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Make<N>
where
    N: MakeService,
{
    taps: Arc<Mutex<Taps>>,
    inner: N,
    _p: PhantomData<C>,
}

/// A middleware that records HTTP taps.
#[derive(Clone, Debug)]
pub struct TapService<S>
where
    S: Service,
{
    taps: Arc<Mutex<Taps>>,
    inner: S,
    _p: PhantomData<C>,
}

pub struct ResponseFuture<S>
where
    S: Service<Error = C::Error>,
{
    ctx: Arc<ctx::Request>,
    classify: Option<C>,
    taps: Arc<Mutex<Taps>>,
    request_open_at: Instant,
    inner: S::Future,
}

#[derive(Debug)]
pub struct RequestBody<T> {
    ctx: Arc<ctx::Request>,

    taps: Option<Arc<Mutex<Taps>>>,

    request_open_at: Instant,

    inner: T,
}

#[derive(Debug)]
pub struct ResponseBody<T> {
    ctx: Arc<ctx::Response>,

    taps: Arc<Mutex<Taps>>,

    request_open_at: Instant,
    response_open_at: Option<Instant>,
    response_first_frame_at: Option<Instant>,
    byte_count: usize,
    frame_count: usize,

    inner: T,
}

// ==== impl Mod ====

impl Mod {
    pub(super) fn new(taps: Arc<Mutex<Taps>>) -> Self {
        Self {
            taps,
            _p: PhantomData,
        }
    }
}

impl<N, A, B> Stack<N> for Mod
where
    A: tower_h2::Body,
    B: tower_h2::Body,
    N: MakeService,
    N::Service: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = h2::Error,
    >,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = <Make<N> as MakeService>::Service;
    type MakeService = Make<N>;

    fn build(&self, inner: N) -> Self::MakeService {
        Make {
            taps: self.taps.clone(),
            inner,
            _p: PhantomData,
        }
    }
}

// ==== impl Make ====

impl<N, A, B> MakeService for Make<N>
where
    A: tower_h2::Body,
    B: tower_h2::Body,
    N: MakeService,
    N::Service: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = h2::Error,
    >,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = TapService<N::Service>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(config)?;
        Ok(TapService {
            taps: self.taps.clone(),
            inner,
        })
    }
}

// === TapService ===

impl<S, A, B> Service for TapService<S>
where
    A: tower_h2::Body,
    B: tower_h2::Body,
    S: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = h2::Error,
    >,
{
    type Request = http::Request<A>;
    type Response = http::Response<ResponseBody<B>>;
    type Error = S::Error;
    type Future = ResponseFuture<S>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let request_open_at = clock::now();
        let req = {
            let (head, inner) = req.into_parts();
            let body = RequestBody {
                taps: self.taps.clone(),
                request_open_at,
                inner,
            };
            http::Request::from_parts(head, body)
        };

        ResponseFuture {
            taps: self.taps.clone(),
            request_open_at,
            inner: self.inner.call(req),
        }
    }
}

impl<S, B> Future for ResponseFuture<S>
where
    B: tower_h2::Body,
    S: Service<Response = http::Response<B>, Error = h2::Error>,
{
    type Item = http::Response<ResponseBody<B>>;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (head, inner) = try_ready!(self.inner.poll()).into_parts();
        let response_open_at = clock::now();
        let body = ResponseBody {
            classify: self.classify.take(),
            taps: self.taps.clone(),
            request_open_at: self.request_open_at,
            response_open_at,
            first_byte_at: None,
            inner,
        };

        let rsp = http::Response::from_parts(head, body);
        Ok(rsp.into())
    }
}

impl<B: tower_h2::Body> RequestBody<B> {
    fn tap_eos(&mut self, trailers: Option<http::HeaderMap>) {
        if let Some(t) = self.taps.take() {
            if let Ok(taps) = t.lock() {
                taps.inspect(event::Event::StreamRequestEnd(
                    Arc::clone(&ctx),
                    event::StreamRequestEnd {
                        request_open_at,
                        request_end_at: Instant::now(),
                    },
                )
            }
        }
    }

    fn tap_err(&mut self, err: h2::Error) -> h2::Error {
        err
    }
}

impl<B: tower_h2::Body> Drop for RequestBody<B> {
    fn drop(&mut self) {
        self.tap_eos(None);
    }
}

impl<B: tower_h2::Body> tower_h2::Body for RequestBody<B> {
    type Data = <B::Data as IntoBuf>::Buf;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let frame = try_ready!(
            self.inner
                .poll_data()
                .map_err(|e| self.tap_err(e))
                .map(|f| f.into_buf())
        );

        if let Some(ref f) = frame {
            self.frame_count += 1;
            self.byte_count += f.remaining();
        }

        if self.inner.is_end_stream() {}

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trailers = try_ready!(self.inner.poll_trailers().map_err(|e| self.tap_err(e)));
        self.tap_eos(trailers.as_ref());
        Ok(Async::Ready(trailers))
    }
}

impl<B: tower_h2::Body> ResponseBody<B> {
    fn tap_eos(&mut self, trailers: Option<&http::HeaderMap>) {}

    fn tap_err(&mut self, err: h2::Error) -> h2::Error {
        err
    }
}

impl<B> tower_h2::Body for ResponseBody<B>
where
    B: tower_h2::Body,
{
    type Data = B::Data;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let frame = try_ready!(
            self.inner
                .poll_data()
                .map_err(|e| self.tap_err(e))
                .map(|f| f.into_buf())
        );

        if self.response_first_frame_at.is_none() {
            self.response_first_frame_at = Some(clock::now());
        }
        if let Some(ref f) = frame {
            self.frame_count += 1;
            self.byte_count += f.remaining();
        }

        if self.inner.is_end_stream() {}

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trailers = try_ready!(self.inner.poll_trailers().map_err(|e| self.tap_err(e)));
        self.tap_eos(trailers.as_ref());
        Ok(Async::Ready(trailers))
    }
}
