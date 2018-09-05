use futures::{Future, Poll};
use http;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_timer::clock;

use linkerd2_metrics::FmtLabels;

use svc::{Service, Stack, MakeService};
use svc::http::classify::{Classify, ClassifyStream};
use svc::http::metrics::{Registry, Metrics};
use svc::http::metrics::body::{RequestBody, ResponseBody};

#[derive(Clone, Debug)]
pub struct Mod<S, C>
where
    S: FmtLabels + Clone + Hash + Eq,
    C: Classify + Clone,
{
    classify: C,
    registry: Arc<Mutex<Registry<S, C::Class>>>,
}

#[derive(Clone, Debug)]
pub struct NewMeasure<N, C>
where
    C: Classify,
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
{
    classify: C,
    registry: Arc<Mutex<Registry<N::Config, C::Class>>>,
    inner: N,
}

#[derive(Clone, Debug)]
pub struct Measure<S: Service, C: Classify> {
    classify: C,
    metrics: Arc<Mutex<Metrics<C::Class>>>,
    inner: S,
}

pub struct ResponseFuture<S: Service, C: ClassifyStream> {
    state: Arc<StreamState<C>>,
    inner: S::Future,
}

#[derive(Debug)]
pub struct RequestBody<T, C: ClassifyStream> {
    state: Option<StreamState<C>>,
    inner: T,
}

#[derive(Debug)]
pub struct ResponseBody<T, C: ClassifyStream> {
    state: Option<StreamState<C>>,
    inner: T,
}

struct StreamState<C: ClassifyStream> {
    init_stamp: Instant,
    metrics: Arc<Mutex<Metrics<C::Class>>>,
    classify: Mutex<C>,
}

// ===== impl Make =====

impl<S, C> Make<S, C>
where
    S: FmtLabels + Clone + Hash + Eq,
    C: Classify + Clone,
{
    pub(super) fn new(
        classify: C,
        registry: Arc<Mutex<Registry<S, C::Class>>>,
    ) -> Self {
        Self { classify, registry }
    }
}

impl<C, N, A, B> Stack<N> for Mod<N::Config, C>
where
    C: Classify + Clone,
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
    N::Service: Service<
        Request = http::Request<RequestBody<A, C::ClassifyStream>>,
        Response = http::Response<B>,
    >,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = <NewMeasure<N, C> as MakeService>::Service;
    type MakeService = NewMeasure<N, C>;

    fn build(&self, inner: N) -> Self::MakeService {
        NewMeasure {
            classify: self.classify.clone(),
            registry: self.registry.clone(),
            inner,
        }
    }
}

// ===== impl NewMeasure =====

impl<C, N, A, B> MakeService for NewMeasure<N, C>
where
    C: Classify + Clone,
    N: MakeService,
    N::Config: FmtLabels + Clone + Hash + Eq,
    N::Service: Service<
        Request = http::Request<RequestBody<A, C::Clas>>,
        Response = http::Response<B>,
    >,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = Measure<N::Service, C>;

    fn make_service(&self, target: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(target)?;

        let metrics = match self.registry.lock() {
            Ok(mut r) => {
                r.by_target.entry(target.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(Metrics::new())))
                    .clone()
            }
            Err(_) => {
                // If the metrics lock was poisoned, just create a one-off
                // structure that won't be reported.
                Arc::new(Mutex::new(Metrics::new()))
            }
        };

        Ok(Measure {
            classify: self.classify.clone(),
            metrics,
            inner
       })
    }
}

// ===== impl Measure =====

impl<C, S, A, B> Service for Measure<S, C>
where
    C: Classify + Clone,
    S: Service<
        Request = http::Request<RequestBody<A>>,
        Response = http::Response<B>,
    >,
{
    type Request = http::Request<A>;
    type Response = http::Response<ResponseBody<B>>;
    type Error = S::Error;
    type Future = ResponseFuture<S, C::ClassifyStream>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        let (head, inner) = req.into_parts();
        let state = Arc::new(StreamState {
            init_stamp: clock::now(),
            classify: Mutex::new(self.classify.classify_stream(&head)),
            metrics: self.metrics.clone(),
        });
        let body = RequestBody::new(state.clone(), inner);
        let req = http::Request::from_parts(head, body);

        ResponseFuture {
            state,
            inner: self.inner.call(req),
        }
    }
}

impl<C, S, B> Future for ResponseFuture<S, C>
where
    S: Service<Response = http::Response<B>>,
    C: ClassifyStream,
{
    type Item = http::Response<ResponseBody<B>>;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (head, inner) = try_ready!(self.inner.poll()).into_parts();
        if let Ok(ref mut classify) = self.state.classify.lock() {
            classify.response(&head);
        }
        let body = ResponseBody::new(self.state.clone(), inner);

        let rsp = http::Response::from_parts(head, body);
        Ok(rsp.into())
    }
}

#[derive(Debug)]
struct State {
    metrics: Arc<Mutex<Metrics>>,
    stream_open_at: Instant,
    rx_bytes: usize,
    tx_bytes: usize,
}

impl<B> RequestBody<B> {
    pub(super) fn new(
        metrics: Arc<Mutex<Metrics>>,
        stream_open_at: Instant,
        inner: B
    ) -> Self {
        Self {
            state: Some(State {
                metrics,
                stream_open_at,
                rx_bytes: 0,
                tx_bytes: 0,
            }),
            inner,
        }
    }
}

impl<B> RequestBody<B>
where
    B: tower_h2::Body,
{
    fn measure_err(&mut self, err: h2::Error) -> h2::Error {
        if let Some(_reason) = err.reason() {
            if let Some(_state) = self.state.take() {
                // TODO
            }
        }

        err
    }
}

impl<B> tower_h2::Body for RequestBody<B>
where
    B: tower_h2::Body,
{
    /// The body chunk type
    type Data = <B::Data as IntoBuf>::Buf;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let frame = try_ready!(
            self.inner
                .poll_data()
                .map_err(|e| self.measure_err(e))
        );
        let frame = frame.map(|f| f.into_buf());

        if let Some(ref _frame) = frame {
            if let Some(ref mut _state) = self.state {
                // TODO
            }
        }

        // If the frame ended the stream, send the end of stream event now,
        // as we may not be polled again.
        if self.is_end_stream() {
            if let Some(_state) = self.state.take() {
                // TODO
            }
        }

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trls = try_ready!(
            self.inner
                .poll_trailers()
                .map_err(|e| self.measure_err(e))
        );

        if let Some(_state) = self.state.take() {
            let _grpc_status = trls.as_ref()
                .and_then(|t| t.get(GRPC_STATUS))
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u32>().ok());
            // TODO
        }

        Ok(Async::Ready(trls))
    }
}

impl<B> ResponseBody<B> {
    pub(super) fn new(
        metrics: Arc<Mutex<Metrics>>,
        stream_open_at: Instant,
        inner: B,
    ) -> Self {
        Self {
            state: Some(State {
                metrics,
                stream_open_at,
                rx_bytes: 0,
                tx_bytes: 0,
            }),
            inner,
        }
    }
}
