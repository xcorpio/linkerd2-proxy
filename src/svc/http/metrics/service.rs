use bytes::IntoBuf;
use futures::{Async, Future, Poll};
use h2;
use http;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio_timer::clock;
use tower_h2;

use svc::either::Either;
use svc::http::classify::{Classify, ClassifyResponse};
use svc::http::metrics::{Metrics, Registry};
use svc::{MakeService, Service, Stack};

const GRPC_STATUS: &str = "grpc-status";

#[derive(Clone, Debug)]
pub struct Mod<T, C>
where
    T: Clone + Hash + Eq,
    C: Classify,
    C::Class: Clone + Hash + Eq,
{
    registry: Arc<Mutex<Registry<T, C::Class>>>,
}

#[derive(Clone, Debug)]
pub struct Make<N, C>
where
    N: MakeService,
    N::Config: Clone + Hash + Eq,
    C: Classify<Error = <N::Service as Service>::Error>,
    C::Class: Clone + Hash + Eq,
{
    registry: Arc<Mutex<Registry<N::Config, C::Class>>>,
    inner: N,
}

#[derive(Clone, Debug)]
pub struct Measure<S, C>
where
    S: Service,
    C: Classify<Error = S::Error>,
    C::Class: Clone + Hash + Eq,
{
    metrics: Option<Arc<Mutex<Metrics<C::Class>>>>,
    inner: S,
}

pub struct ResponseFuture<S, C>
where
    S: Service<Error = C::Error>,
    C: ClassifyResponse,
    C::Class: Clone + Hash + Eq,
{
    classify: Option<C>,
    metrics: Option<Arc<Mutex<Metrics<C::Class>>>>,
    stream_open_at: Instant,
    inner: S::Future,
}

#[derive(Debug)]
pub struct RequestBody<T, C>
where
    C: Clone + Hash + Eq,
{
    metrics: Option<Arc<Mutex<Metrics<C>>>>,
    inner: T,
}

#[derive(Debug)]
pub struct ResponseBody<T, C>
where
    C: ClassifyResponse + Debug + Send + Sync + 'static,
    C::Class: Clone + Hash + Eq,
{
    class_or_classify: Either<C::Class, Option<C>>,
    metrics: Option<Arc<Mutex<Metrics<C::Class>>>>,
    stream_open_at: Instant,
    inner: T,
}

// ===== impl Make =====

impl<T, C> Mod<T, C>
where
    T: Clone + Hash + Eq,
    C: Classify,
    C::Class: Clone + Hash + Eq,
    C::ClassifyResponse: Send + Sync + 'static,
{
    pub(super) fn new(registry: Arc<Mutex<Registry<T, C::Class>>>) -> Self {
        Self { registry }
    }
}

impl<N, A, B, C> Stack<N> for Mod<N::Config, C>
where
    N: MakeService,
    N::Config: Clone + Hash + Eq,
    N::Service: Service<
        Request = http::Request<RequestBody<A, C::Class>>,
        Response = http::Response<B>,
        Error = C::Error,
    >,
    C: Classify,
    C::ClassifyResponse: Debug + Send + Sync + 'static,
    C::Class: Clone + Hash + Eq,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = <Make<N, C> as MakeService>::Service;
    type MakeService = Make<N, C>;

    fn build(&self, inner: N) -> Self::MakeService {
        Make {
            registry: self.registry.clone(),
            inner,
        }
    }
}

// ===== impl Make =====

impl<N, A, B, C> MakeService for Make<N, C>
where
    N: MakeService,
    N::Config: Clone + Hash + Eq,
    N::Service: Service<
        Request = http::Request<RequestBody<A, C::Class>>,
        Response = http::Response<B>,
        Error = C::Error,
    >,
    C: Classify,
    C::Class: Clone + Hash + Eq,
    C::ClassifyResponse: Debug + Send + Sync + 'static,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = Measure<N::Service, C>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        let inner = self.inner.make_service(config)?;

        let metrics = match self.registry.lock() {
            Ok(mut r) => Some(
                r.by_config
                    .entry(config.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(Metrics::default())))
                    .clone(),
            ),
            Err(_) => None,
        };

        Ok(Measure { metrics, inner })
    }
}

// ===== impl Measure =====

impl<C, S, A, B> Service for Measure<S, C>
where
    S: Service<
        Request = http::Request<RequestBody<A, C::Class>>,
        Response = http::Response<B>,
        Error = C::Error,
    >,
    C: Classify,
    C::Class: Clone + Hash + Eq,
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
        let classify = req.extensions().get::<C::ClassifyResponse>().cloned();

        let (head, inner) = req.into_parts();
        let body = RequestBody {
            metrics: self.metrics.clone(),
            inner,
        };
        let req = http::Request::from_parts(head, body);

        ResponseFuture {
            classify,
            metrics: self.metrics.clone(),
            stream_open_at: clock::now(),
            inner: self.inner.call(req),
        }
    }
}

impl<C, S, B> Future for ResponseFuture<S, C>
where
    S: Service<Response = http::Response<B>, Error = C::Error>,
    C: ClassifyResponse + Debug + Send + Sync + 'static,
    C::Class: Clone + Hash + Eq,
{
    type Item = http::Response<ResponseBody<B, C>>;
    type Error = S::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        let (head, inner) = try_ready!(self.inner.poll()).into_parts();

        let class_or_classify = match self.classify.take() {
            Some(mut classify) => match classify.start(&head) {
                Some(class) => Either::A(class),
                None => Either::B(Some(classify)),
            },
            None => Either::B(None),
        };

        let body = ResponseBody {
            class_or_classify,
            metrics: self.metrics.clone(),
            stream_open_at: self.stream_open_at,
            inner,
        };
        let rsp = http::Response::from_parts(head, body);

        Ok(rsp.into())
    }
}

impl<B, C> RequestBody<B, C>
where
    B: tower_h2::Body,
    C: Clone + Hash + Eq,
{
    fn measure_err(&mut self, err: h2::Error) -> h2::Error {
        if let Some(_reason) = err.reason() {
            if let Some(_state) = self.metrics.take() {
                // TODO
            }
        }

        err
    }
}

impl<B, C> tower_h2::Body for RequestBody<B, C>
where
    B: tower_h2::Body,
    C: Clone + Hash + Eq,
{
    /// The body chunk type
    type Data = <B::Data as IntoBuf>::Buf;

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn poll_data(&mut self) -> Poll<Option<Self::Data>, h2::Error> {
        let frame = try_ready!(self.inner.poll_data().map_err(|e| self.measure_err(e)));
        let frame = frame.map(|f| f.into_buf());

        Ok(Async::Ready(frame))
    }

    fn poll_trailers(&mut self) -> Poll<Option<http::HeaderMap>, h2::Error> {
        let trls = try_ready!(self.inner.poll_trailers().map_err(|e| self.measure_err(e)));

        Ok(Async::Ready(trls))
    }
}
