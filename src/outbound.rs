#![allow(unused_imports)]

use std::{error, fmt};
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::time::Duration;
use std::sync::Arc;

use http;
use futures::{Async, Poll, Stream};
use tower_buffer::{Buffer, SpawnError};
use tower_in_flight_limit::InFlightLimit;
use tower_h2;

use bind::Protocol;
use ctx;
use proxy::{self, resolve::{self, Resolve as _Resolve}};
use proxy::http::{balance, h1, router};
use svc::{self, Layer as _Layer};
use telemetry::http::service::{ResponseBody as SensorBody};
use timeout::Timeout;
use transport::{DnsNameAndPort, Host, HostAndPort};

#[derive(Clone, Debug, Default)]
pub struct Recognize {}

#[derive(Clone, Debug)]
pub struct Layer<T, M, L>
{
    inner: L,
    router_capacity: usize,
    router_max_idle_age: Duration,
    _p: PhantomData<fn() -> (T, M)>,
}

#[derive(Clone, Debug)]
pub struct Make<R, M>
where
    R: resolve::Resolve<DnsNameAndPort>,
    M: svc::Make<R::Endpoint>
{
    resolve: Resolve<R>,
    bind_timeout: Duration,
    router_capacity: usize,
    router_max_idle_age: Duration,
    make_endpoint: M,
}

const MAX_IN_FLIGHT: usize = 10_000;

/// This default is used by Finagle.
const DEFAULT_DECAY: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target  {
    destination: Destination,
    protocol: Protocol,
}

/// Describes a destination for HTTP requests.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Destination {
    /// A logical, lazily-bound endpoint.
    Name(DnsNameAndPort),

    /// A single, bound endpoint.
    Addr(SocketAddr),
}

#[derive(Clone, Debug)]
struct Resolve<R: resolve::Resolve<DnsNameAndPort>>(R);

#[derive(Debug)]
pub enum Resolution<R: resolve::Resolution> {
    Name(R),
    Addr(Option<SocketAddr>),
}


impl<T, L, M, A, B> Layer<T, M, L>
where
    T: fmt::Debug,
    L: svc::Layer<M>,
    M: svc::Make<T>,
    M::Output: svc::Service<
        Request = http::Request<A>,
        Response = http::Request<B>,
    >,
{
    pub fn new<R>(
        resolve: R,
        bind_timeout: Duration,
        router_capacity: usize,
        router_max_idle_age: Duration,
    )
        -> Self
    where
        R: resolve::Resolve<DnsNameAndPort, Endpoint = T> + Clone,
    {
        let inner = proxy::timeout::Layer::new(bind_timeout)
            .and_then(proxy::buffer::Layer::default())
            .and_then(proxy::http::balance::Layer::new(Resolve(resolve)));
        Self {
            inner,
            router_capacity,
            router_max_idle_age,
        }
    }
}

impl<T, L, M, A, B> svc::Layer<M> for Layer<T, M, L>
where
    L: svc::Layer<M>,
    M: svc::Make<T> + Clone + Send + Sync + 'static,
    M::Output: Send + Sync,
    M::Output: svc::Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
    <M::Output as svc::Service>::Future: Send,
    M::Error: Send,
    A: tower_h2::Body + Send + 'static,
    B: tower_h2::Body + Send + 'static,
{
    type Bound = L::Bound;

    fn bind(&self, make: M) -> Self::Bound {
        self.inner.bind(make)
    }
}

impl<B> router::Recognize<http::Request<B>> for Recognize {
    type Target = Target;

    // Route the request by its destination AND PROTOCOL. This prevents HTTP/1
    // requests from being routed to HTTP/2 servers, and vice versa.
    fn recognize(&self, req: &http::Request<B>) -> Option<Self::Target> {
        let destination = Self::destination(req)?;
        let protocol = Protocol::detect(req);
        Some(Target { destination, protocol })
    }
}

impl Recognize {
    /// TODO: Return error when `HostAndPort::normalize()` fails.
    /// TODO: Use scheme-appropriate default port.
    fn normalize(authority: &http::uri::Authority) -> Option<HostAndPort> {
        const DEFAULT_PORT: Option<u16> = Some(80);
        HostAndPort::normalize(authority, DEFAULT_PORT).ok()
    }

    /// Determines the logical host:port of the request.
    ///
    /// If the parsed URI includes an authority, use that. Otherwise, try to load the
    /// authority from the `Host` header.
    ///
    /// The port is either parsed from the authority or a default of 80 is used.
    fn host_port<B>(req: &http::Request<B>) -> Option<HostAndPort> {
        // Note: Calls to `normalize` cannot be deduped without cloning `authority`.
        req.uri()
            .authority_part()
            .and_then(Self::normalize)
            .or_else(|| {
                h1::authority_from_host(req)
                    .and_then(|h| Self::normalize(&h))
            })
    }

    /// Determines the destination for a request.
    ///
    /// Typically, a request's authority is used to produce a `Destination`. If the
    /// authority addresses a DNS name, a `Destination::Name` is returned; and, otherwise,
    /// it addresses a fixed IP address and a `Destination::Addr` is returned. The port is
    /// inferred if not specified in the authority.
    ///
    /// If no authority is available, the `SO_ORIGINAL_DST` socket option is checked. If
    /// it's available, it is used to return a `Destination::Addr`. This socket option is
    /// typically set by `iptables(8)` in containerized environments like Kubernetes (as
    /// configured by the `proxy-init` program).
    ///
    /// If none of this information is available, no `Destination` is returned.
    fn destination<B>(req: &http::Request<B>) -> Option<Destination> {
        match Self::host_port(req) {
            Some(HostAndPort { host: Host::DnsName(host), port }) => {
                let dst = DnsNameAndPort { host, port };
                Some(Destination::Name(dst))
            }

            Some(HostAndPort { host: Host::Ip(ip), port }) => {
                let dst = SocketAddr::from((ip, port));
                Some(Destination::Addr(dst))
            }

            None => {
                req.extensions()
                    .get::<Arc<ctx::transport::Server>>()
                    .and_then(|ctx| ctx.orig_dst_if_not_local())
                    .map(Destination::Addr)
            }
        }
    }
}

impl<R> resolve::Resolve<Target> for Resolve<R>
where
    R: resolve::Resolve<DnsNameAndPort>,
    R::Endpoint: From<SocketAddr>,
{
    type Endpoint = R::Endpoint;
    type Resolution = Resolution<R::Resolution>;

    fn resolve(&self, t: &Target) -> Self::Resolution {
        match t.destination {
            Destination::Name(ref name) => Resolution::Name(self.0.resolve(&name)),
            Destination::Addr(ref addr) => Resolution::Addr(Some(*addr)),
        }
    }
}

impl<R> resolve::Resolution for Resolution<R>
where
    R: resolve::Resolution,
    R::Endpoint: From<SocketAddr>,
{
    type Endpoint = R::Endpoint;
    type Error = R::Error;

    fn poll(&mut self) -> Poll<resolve::Update<Self::Endpoint>, Self::Error> {
        match self {
            Resolution::Name(ref mut res ) => res.poll(),
            Resolution::Addr(ref mut addr) => match addr.take() {
                Some(addr) => {
                    let up = resolve::Update::Make(addr, addr.into());
                    Ok(Async::Ready(up))
                }
                None => Ok(Async::NotReady),
            }
        }
    }
}

struct Dst(Destination);

impl fmt::Display for Dst {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            Destination::Name(ref name) => {
                write!(f, "{}:{}", name.host, name.port)
            }
            Destination::Addr(ref addr) => addr.fmt(f),
        }
    }
}
