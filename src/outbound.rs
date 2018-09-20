#![allow(unused_imports)]

use std::{error, fmt};
use std::net::SocketAddr;
use std::time::Duration;
use std::sync::Arc;

use http;
use futures::{Async, Poll};
use tower_balance::{choose, load, Balance};
use tower_buffer::{Buffer, SpawnError};
use tower_discover::{Change, Discover};
use tower_in_flight_limit::InFlightLimit;
use tower_h2;
use tower_h2_balance::{PendingUntilFirstData, PendingUntilFirstDataBody};

use bind::{self, Protocol};
use control::destination::{self, Endpoint, Resolution};
use ctx;
use proxy::{self, http::h1};
use proxy::http::router;
use svc;
use telemetry::http::service::{ResponseBody as SensorBody};
use timeout::Timeout;
use transport::{DnsNameAndPort, Host, HostAndPort};

pub struct Recognize;

#[derive(Clone, Debug)]
pub struct Layer {
    resolver: destination::Resolver,
    bind_timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct Make<E: svc::Make<Endpoint>> {
    resolver: destination::Resolver,
    bind_timeout: Duration,
    make_endpoint: E,
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

impl Layer {
    pub fn new(
        resolver:  destination::Resolver,
        bind_timeout: Duration
    ) -> Self {
        Self {
            resolver,
            bind_timeout,
        }
    }
}

impl<E, A, B> svc::Layer<E> for Layer
where
    E: svc::Make<Endpoint> + Clone + Send + Sync + 'static,
    E::Output: Send + Sync,
    E::Output: svc::Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
    <E::Output as svc::Service>::Future: Send,
    E::Error: Send,
    A: tower_h2::Body + Send + 'static,
    B: tower_h2::Body + Send + 'static,
{
    type Bound = Make<E>;

    fn bind(&self, make_endpoint: E) -> Self::Bound {
        Make {
            resolver: self.resolver.clone(),
            bind_timeout: self.bind_timeout,
            make_endpoint,
        }
    }
}

type Bal<E> = Balance<
    load::WithPeakEwma<Discovery<E>, PendingUntilFirstData>,
    choose::PowerOfTwoChoices,
>;

impl<E, A, B> svc::Make<Target> for Make<E>
where
    E: svc::Make<Endpoint> + Clone + Send + 'static,
    E::Output: Send,
    E::Output: svc::Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
    <E::Output as svc::Service>::Future: Send,
    E::Error: Send,
    A: tower_h2::Body + Send + 'static,
    B: tower_h2::Body + Send + 'static,
{
    type Output = InFlightLimit<Timeout<Buffer<Bal<E>>>>;
    type Error = SpawnError<Bal<E>>;

    fn make(&self, target: &Target) -> Result<Self::Output, Self::Error> {
        let &Target { ref destination, ref protocol } = target;
        debug!("building outbound {:?} client to {:?}", protocol, destination);

        let resolve = match *destination {
            Destination::Name(ref authority) =>
                Discovery::Name(self.resolver.resolve(authority, self.make_endpoint.clone())),
            Destination::Addr(addr) => Discovery::Addr(Some((addr, self.make_endpoint.clone()))),
        };

        let balance = {
            let instrument = PendingUntilFirstData::default();
            let loaded = load::WithPeakEwma::new(resolve, DEFAULT_DECAY, instrument);
            Balance::p2c(loaded)
        };

        let log = ::logging::proxy().client("out", Dst(destination.clone()))
            .with_protocol(protocol.clone());
        let buffer = Buffer::new(balance, &log.executor())?;

        let timeout = Timeout::new(buffer, self.bind_timeout);

        Ok(InFlightLimit::new(timeout, MAX_IN_FLIGHT))
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

pub enum Discovery<M: svc::Make<Endpoint>> {
    Name(Resolution<M>),
    Addr(Option<(SocketAddr, M)>),
}

impl<M> Discover for Discovery<M>
where
    M: svc::Make<Endpoint>,
    M::Output: svc::Service,
{
    type Key = SocketAddr;
    type Request = <M::Output as svc::Service>::Request;
    type Response = <M::Output as svc::Service>::Response;
    type Error = <M::Output as svc::Service>::Error;
    type Service = M::Output;
    type DiscoverError = M::Error;

    fn poll(&mut self) -> Poll<Change<Self::Key, Self::Service>, Self::DiscoverError> {
        match *self {
            Discovery::Name(ref mut w) => w.poll(),
            Discovery::Addr(ref mut opt) => {
                // This "discovers" a single address for an external service
                // that never has another change. This can mean it floats
                // in the Balancer forever. However, when we finally add
                // circuit-breaking, this should be able to take care of itself,
                // closing down when the connection is no longer usable.
                if let Some((addr, mut stack)) = opt.take() {
                    let svc = stack.make(&addr.into())?;
                    Ok(Async::Ready(Change::Insert(addr, svc)))
                } else {
                    Ok(Async::NotReady)
                }
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
