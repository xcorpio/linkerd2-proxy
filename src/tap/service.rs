use bytes::{Buf, IntoBuf};
use futures::{Async, Future, Poll};
use h2;
use http;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_timer::clock;
use tower_h2;

use super::{ctx, event, Taps};
use ctx::transport::{Client as ClientCtx, Server as ServerCtx};
use svc::{NewClient, Service, Stack};

/// A stack module that wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Mod {
    taps: Arc<Mutex<Taps>>,
}

/// Wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Make<N>
where
    N: NewClient,
{
    taps: Arc<Mutex<Taps>>,
    inner: N,
}

/// A middleware that records HTTP taps.
#[derive(Clone, Debug)]
pub struct TapService<S>
where
    S: Service,
{
    ctx: Arc<ClientCtx>,
    taps: Arc<Mutex<Taps>>,
    inner: S,
}

pub struct ResponseFuture<S>
where
    S: Service,
{
    state: Option<RequestState>,
    inner: S::Future,
}

#[derive(Debug)]
pub struct RequestBody<B> {
    state: Option<RequestState>,
    inner: B,
}

#[derive(Clone, Debug)]
struct RequestState {
    ctx: Arc<ctx::Request>,
    taps: Option<Arc<Mutex<Taps>>>,
    request_open_at: Instant,
    byte_count: u64,
    frame_count: u32,
}

#[derive(Debug)]
pub struct ResponseBody<B> {
    state: Option<ResponseState>,
    inner: B,
}

#[derive(Debug)]
struct ResponseState {
    ctx: Arc<ctx::Response>,
    taps: Option<Arc<Mutex<Taps>>>,
    request_open_at: Instant,
    response_open_at: Instant,
    response_first_frame_at: Option<Instant>,
    byte_count: u64,
    frame_count: u32,
}

// ==== impl Mod ====

impl Mod {
    pub(super) fn new(taps: Arc<Mutex<Taps>>) -> Self {
        Self { taps }
    }
}

impl<N, A, B> Stack<N> for Mod
where
    A: tower_h2::Body,
    B: tower_h2::Body,
    N: NewClient<Config = Arc<ClientCtx>>,
    N::Service: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = h2::Error,
    >,
{
    type Config = Arc<ClientCtx>;
    type Error = N::Error;
    type Service = <Make<N> as NewClient>::Service;
    type NewClient = Make<N>;

    fn build(&self, inner: N) -> Self::NewClient {
        Make {
            taps: self.taps.clone(),
            inner,
        }
    }
}

// ==== impl Make ====

impl<N, A, B> NewClient for Make<N>
where
    A: tower_h2::Body,
    B: tower_h2::Body,
    N: NewClient<Config = Arc<ClientCtx>>,
    N::Service: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
        Error = h2::Error,
    >,
{
    type Config = Arc<ClientCtx>;
    type Error = N::Error;
    type Service = TapService<N::Service>;

    fn make_service(&self, ctx: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(&ctx)?;
        Ok(TapService {
            ctx: ctx.clone(),
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
        let ctx = req
            .extensions()
            .get::<Arc<ServerCtx>>()
            .map(|srv| ctx::Request::new(&req, &srv, &self.ctx));

        if let Some(ctx) = ctx.as_ref() {
            if let Ok(mut taps) = self.taps.lock() {
                taps.inspect(&event::Event::StreamRequestOpen(ctx.clone()));
            }
        }

        let mut state = ctx.as_ref().map(|ctx| RequestState {
            ctx: ctx.clone(),
            taps: Some(self.taps.clone()),
            request_open_at,
            byte_count: 0,
            frame_count: 0,
        });

        if req.body().is_end_stream() {
            if let Some(mut state) = state.take() {
                state.tap_eos(Some(req.headers()));
            }
        }

        let req = {
            let (head, inner) = req.into_parts();
            let state = state.clone();
            http::Request::from_parts(head, RequestBody { state, inner })
        };

        ResponseFuture {
            state,
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
    type Error = h2::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let rsp = match self.inner.poll() {
            Ok(Async::NotReady) => return Ok(Async::NotReady),
            Ok(Async::Ready(rsp)) => rsp,
            Err(e) => {
                if let Some(mut state) = self.state.take() {
                    state.tap_err(&e);
                }
                return Err(e);
            }
        };
        let response_open_at = clock::now();

        let state = if let Some(mut req) = self.state.take() {
            if let Some(taps) = req.taps.take() {
                let ctx = ctx::Response::new(&rsp, &req.ctx);

                if let Ok(mut taps) = taps.lock() {
                    taps.inspect(&event::Event::StreamResponseOpen(
                        ctx.clone(),
                        event::StreamResponseOpen {
                            request_open_at: req.request_open_at,
                            response_open_at,
                        },
                    ));
                }

                let mut state = ResponseState {
                    ctx,
                    taps: Some(taps),
                    request_open_at: req.request_open_at,
                    response_open_at,
                    response_first_frame_at: None,
                    byte_count: 0,
                    frame_count: 0,
                };

                if rsp.body().is_end_stream() {
                    state.tap_eos(Some(rsp.headers()));
                }

                Some(state)
            } else {
                None
            }
        } else {
            None
        };


        let rsp = {
            let (head, inner) = rsp.into_parts();
            http::Response::from_parts(head, ResponseBody { state, inner })
        };
        Ok(rsp.into())
    }
}

impl RequestState {
    fn tap_eos(&mut self, _: Option<&http::HeaderMap>) {
        if let Some(t) = self.taps.take() {
            if let Ok(mut taps) = t.lock() {
                taps.inspect(&event::Event::StreamRequestEnd(
                    self.ctx.clone(),
                    event::StreamRequestEnd {
                        request_open_at: self.request_open_at,
                        request_end_at: clock::now(),
                    },
                ));
            }
        }
    }

    fn tap_err(&mut self, err: &h2::Error) {
        if let Some(t) = self.taps.take() {
            if let Ok(mut taps) = t.lock() {
                taps.inspect(&event::Event::StreamRequestFail(
                    self.ctx.clone(),
                    event::StreamRequestFail {
                        request_open_at: self.request_open_at,
                        request_fail_at: clock::now(),
                        error: err.reason().unwrap_or(h2::Reason::INTERNAL_ERROR),
                    },
                ));
            }
        }
    }
}

impl Drop for RequestState {
    fn drop(&mut self) {
        // TODO this should be recorded as a cancelation if the stream didn't end.
        self.tap_eos(None);
    }
}

impl<B: tower_h2::Body> RequestBody<B> {
    fn tap_eos(&mut self, trailers: Option<&http::HeaderMap>) {
        if let Some(mut state) = self.state.take() {
            state.tap_eos(trailers);
        }
    }

    fn tap_err(&mut self, e: h2::Error) -> h2::Error {
        if let Some(mut state) = self.state.take() {
            state.tap_err(&e);
        }
        e
    }
}

impl<B: tower_h2::Body> tower_h2::Body for RequestBody<B> {
    type Data = <B::Data as IntoBuf>::Buf;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let poll_frame = self.inner.poll_data().map_err(|e| self.tap_err(e));
        let frame = try_ready!(poll_frame).map(|f| f.into_buf());

        if let Some(s) = self.state.as_mut() {
            if let Some(ref f) = frame {
                s.frame_count += 1;
                s.byte_count += f.remaining();
            }
        }

        if self.inner.is_end_stream() {
            self.tap_eos(None);
        }

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trailers = try_ready!(self.inner.poll_trailers().map_err(|e| self.tap_err(e)));
        self.tap_eos(trailers.as_ref());
        Ok(Async::Ready(trailers))
    }
}

impl ResponseState {
    fn grpc_status(t: &http::HeaderMap) -> Option<u32> {
        t.get("grpc-status")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u32>().ok())
    }

    fn tap_eos(&mut self, trailers: Option<&http::HeaderMap>) {
        if let Some(t) = self.taps.take() {
            let response_end_at = clock::now();
            if let Ok(mut taps) = t.lock() {
                taps.inspect(&event::Event::StreamResponseEnd(
                    self.ctx.clone(),
                    event::StreamResponseEnd {
                        request_open_at: self.request_open_at,
                        response_open_at: self.response_open_at,
                        response_first_frame_at: self
                            .response_first_frame_at
                            .unwrap_or(response_end_at),
                        response_end_at,
                        grpc_status: trailers.and_then(Self::grpc_status),
                    },
                ));
            }
        }
    }

    fn tap_err(&mut self, err: &h2::Error) {
        if let Some(t) = self.taps.take() {
            if let Ok(mut taps) = t.lock() {
                taps.inspect(&event::Event::StreamResponseFail(
                    self.ctx.clone(),
                    event::StreamResponseFail {
                        request_open_at: self.request_open_at,
                        response_open_at: self.response_open_at,
                        response_first_frame_at: self.response_first_frame_at,
                        response_fail_at: clock::now(),
                        error: err.reason().unwrap_or(h2::Reason::INTERNAL_ERROR),
                    },
                ));
            }
        }
    }
}

impl Drop for ResponseState {
    fn drop(&mut self) {
        // TODO this should be recorded as a cancelation if the stream didn't end.
        self.tap_eos(None);
    }
}

impl<B: tower_h2::Body> ResponseBody<B> {
    fn tap_eos(&mut self, trailers: Option<&http::HeaderMap>) {
        if let Some(mut state) = self.state.take() {
            state.tap_eos(trailers);
        }
    }

    fn tap_err(&mut self, e: h2::Error) -> h2::Error {
        if let Some(mut state) = self.state.take() {
            state.tap_err(&e);
        }
        e
    }
}

impl<B> tower_h2::Body for ResponseBody<B>
where
    B: tower_h2::Body,
{
    type Data = <B::Data as IntoBuf>::Buf;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let poll_frame = self.inner.poll_data().map_err(|e| self.tap_err(e));
        let frame = try_ready!(poll_frame).map(|f| f.into_buf());

        if let Some(ref mut s) = self.state.as_mut() {
            if s.response_first_frame_at.is_none() {
                s.response_first_frame_at = Some(clock::now());
            }
            if let Some(ref f) = frame {
                s.frame_count += 1;
                s.byte_count += f.remaining();
            }
        }

        if self.inner.is_end_stream() {
            self.tap_eos(None);
        }

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trailers = try_ready!(self.inner.poll_trailers().map_err(|e| self.tap_err(e)));
        self.tap_eos(trailers.as_ref());
        Ok(Async::Ready(trailers))
    }
}
