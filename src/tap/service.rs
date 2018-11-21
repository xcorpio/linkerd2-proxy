use bytes::{Buf, IntoBuf};
use futures::{Async, Future, Poll, Stream};
use h2;
use http;
use std::collections::VecDeque;
use std::sync::Weak;
use tower_h2::Body as Payload;

use proxy::http::HasH2Reason;
use super::{Register, Tap, TapBody, TapResponse};
use svc;

/// A stack module that wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Layer<R: Register> {
    registry: R,
}

/// Wraps services to record taps.
#[derive(Clone, Debug)]
pub struct Stack<R: Register, T> {
    registry: R,
    inner: T,
}

/// A middleware that records HTTP taps.
#[derive(Clone, Debug)]
pub struct Service<R, S, T> {
    subscription_rx: R,
    subscriptions: VecDeque<Weak<S>>,
    inner: T,
}

#[derive(Debug, Clone)]
pub struct ResponseFuture<F, T: TapResponse> {
    inner: F,
    taps: VecDeque<T>,
}

#[derive(Debug)]
pub struct Body<B, T: TapBody> {
    inner: B,
    taps: VecDeque<T>,
}

// === Layer ===

pub fn layer<R>(registry: R) -> Layer<R>
where
    R: Register + Clone,
{
    Layer { registry }
}

impl<R, T, M> svc::Layer<T, T, M> for Layer<R>
where
    R: Register + Clone,
    M: svc::Stack<T>,
{
    type Value = <Stack<R, M> as svc::Stack<T>>::Value;
    type Error = M::Error;
    type Stack = Stack<R, M>;

    fn bind(&self, inner: M) -> Self::Stack {
        Stack {
            inner,
            registry: self.registry.clone(),
        }
    }
}

// === Stack ===

impl<R, T, M> svc::Stack<T> for Stack<R, M>
where
    R: Register + Clone,
    M: svc::Stack<T>,
{
    type Value = Service<R::Taps, R::Tap, M::Value>;
    type Error = M::Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let inner = self.inner.make(&target)?;
        let subscription_rx = self.registry.register();
        Ok(Service {
            subscription_rx,
            subscriptions: VecDeque::default(),
            inner,
        })
    }
}

// === Service ===

impl<R, S, T, A, B> svc::Service<http::Request<A>> for Service<R, S, T>
where
    R: Stream<Item = Weak<S>>,
    S: Tap,
    T: svc::Service<
        http::Request<Body<A, S::TapRequestBody>>,
        Response = http::Response<B>,
    >,
    T::Error: HasH2Reason,
    A: Payload,
    B: Payload,
{
    type Response = http::Response<Body<B, S::TapResponseBody>>;
    type Error = T::Error;
    type Future = ResponseFuture<T::Future, S::TapResponse>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        while let Ok(Async::Ready(Some(s))) = self.subscription_rx.poll() {
            self.subscriptions.push_back(s);
        }
        self.subscriptions.retain(|s| s.upgrade().is_some());
        self.inner.poll_ready()
    }

    fn call(&mut self, req: http::Request<A>) -> Self::Future {
        let taps = self.subscriptions.iter().filter_map(|s| {
            s.upgrade().and_then(|s| s.tap(&req))
        });

        let req_taps = VecDeque::with_capacity(self.subscriptions.len());
        let rsp_taps = VecDeque::with_capacity(self.subscriptions.len());
        for (req_tap, rsp_tap) in taps {
            req_taps.push_back(req_tap);
            rsp_taps.push_back(rsp_tap);
        }

        let req = {
            let (head, inner) = req.into_parts();
            let taps = req_taps;
            http::Request::from_parts(head, Body { inner, taps })
        };

        let inner = self.inner.call(req);
        ResponseFuture {
            inner,
            taps: rsp_taps,
        }
    }
}

impl<B, F, T> Future for ResponseFuture<F, T>
where
    B: Payload,
    F: Future<Item = http::Response<B>>,
    F::Error: HasH2Reason,
    T: TapResponse,
{
    type Item = http::Response<Body<B, T::TapBody>>;
    type Error = F::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let rsp = try_ready!(self.inner.poll().map_err(|e| self.tap_err(e)));

        let taps = self.taps.drain(..).map(|t| t.tap(&rsp)).collect();
        let rsp = {
            let (head, inner) = rsp.into_parts();
            let body = Body { inner, taps };
            http::Response::from_parts(head, body)
        };

        Ok(rsp.into())
    }
}

impl<B, F, T> ResponseFuture<F, T>
where
    B: Payload,
    F: Future<Item = http::Response<B>>,
    F::Error: HasH2Reason,
    T: TapResponse,
{
    fn tap_err(&mut self, error: F::Error) -> F::Error {
        while let Some(tap) = self.taps.pop_front() {
            tap.fail(&error);
        }

        error
    }
}

// === Body ===

impl<B: Payload, T: TapBody> Payload for Body<B, T> {
    type Data = <B::Data as IntoBuf>::Buf;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let poll_frame = self.inner.poll_data().map_err(|e| self.tap_err(e));
        let frame = try_ready!(poll_frame).map(|f| f.into_buf());

        if let Some(ref f) = frame.as_ref() {
            for ref mut tap in self.taps.iter_mut() {
                tap.data(f);
            }
        }

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trailers = try_ready!(self.inner.poll_trailers().map_err(|e| self.tap_err(e)));
        self.tap_eos(trailers.as_ref());
        Ok(Async::Ready(trailers))
    }
}

impl<B, T: TapBody> Body<B, T> {

    fn tap_eos(&mut self, trailers: Option<&http::HeaderMap>) {
        while let Some(tap) = self.taps.pop_front() {
            tap.end(trailers);
        }
    }

    fn tap_err(&mut self, error: h2::Error) -> h2::Error {
        while let Some(tap) = self.taps.pop_front() {
            tap.fail(&error);
        }

        error
    }
}

impl<B, T: TapBody> Drop for Body<B, T> {
    fn drop(&mut self) {
        // TODO this should be recorded as a cancelation if the stream didn't end.
        self.tap_eos(None);
    }
}

