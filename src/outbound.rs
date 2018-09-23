use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use http;
use futures::{Async, Poll};

use ctx;
use proxy::resolve;
use proxy::http::{balance, h1, router, Settings};
use transport::{DnsNameAndPort, Host, HostAndPort};

#[derive(Clone, Debug, Default)]
pub struct Recognize {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Destination {
    pub name_or_addr: NameOrAddr,
    pub settings: Settings,
    _p: (),
}

pub struct Endpoint {
    pub settings: Settings,
    _p: (),
}

/// Describes a destination for HTTP requests.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NameOrAddr {
    /// A logical, lazily-bound endpoint.
    Name(DnsNameAndPort),

    /// A single, bound endpoint.
    Addr(SocketAddr),
}

#[derive(Clone, Debug)]
pub struct Resolve<R: resolve::Resolve<DnsNameAndPort>>(R);

#[derive(Debug)]
pub enum Resolution<R: resolve::Resolution> {
    Name(R),
    Addr(Option<SocketAddr>),
}


impl<B> router::Recognize<http::Request<B>> for Recognize {
    type Target = Destination;

    // Route the request by its destination AND PROTOCOL. This prevents HTTP/1
    // requests from being routed to HTTP/2 servers, and vice versa.
    fn recognize(&self, req: &http::Request<B>) -> Option<Self::Target> {
        let name_or_addr = Self::name_or_addr(req)?;
        let settings = Settings::detect(req);
        Some(Destination { name_or_addr, settings, _p: PhantomData })
    }
}

impl Recognize {
    pub fn new() -> Self {
        Self {}
    }

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
    /// Typically, a request's authority is used to produce a `NameOrAddr`. If the
    /// authority addresses a DNS name, a `NameOrAddr::Name` is returned; and, otherwise,
    /// it addresses a fixed IP address and a `NameOrAddr::Addr` is returned. The port is
    /// inferred if not specified in the authority.
    ///
    /// If no authority is available, the `SO_ORIGINAL_DST` socket option is checked. If
    /// it's available, it is used to return a `NameOrAddr::Addr`. This socket option is
    /// typically set by `iptables(8)` in containerized environments like Kubernetes (as
    /// configured by the `proxy-init` program).
    ///
    /// If none of this information is available, no `NameOrAddr` is returned.
    fn name_or_addr<B>(req: &http::Request<B>) -> Option<NameOrAddr> {
        match Self::host_port(req) {
            Some(HostAndPort { host: Host::DnsName(host), port }) => {
                let name_or_addr = DnsNameAndPort { host, port };
                Some(NameOrAddr::Name(name_or_addr))
            }

            Some(HostAndPort { host: Host::Ip(ip), port }) => {
                let name_or_addr = SocketAddr::from((ip, port));
                Some(NameOrAddr::Addr(name_or_addr))
            }

            None => {
                req.extensions()
                    .get::<Arc<ctx::transport::Server>>()
                    .and_then(|ctx| ctx.orig_name_or_addr_if_not_local())
                    .map(NameOrAddr::Addr)
            }
        }
    }
}

impl<R> Resolve<R>
where
    R: resolve::Resolve<DnsNameAndPort>,
    R::Endpoint: From<SocketAddr>,
{
    pub fn new(resolve: R) -> Self {
        Resolve(resolve)
    }
}


impl<R> resolve::Resolve<Destination> for Resolve<R>
where
    R: resolve::Resolve<DnsNameAndPort>,
    R::Endpoint: From<SocketAddr>,
{
    type Endpoint = R::Endpoint;
    type Resolution = Resolution<R::Resolution>;

    fn resolve(&self, t: &Destination) -> Self::Resolution {
        match t.name_or_addr {
            NameOrAddr::Name(ref name) => Resolution::Name(self.0.resolve(&name)),
            NameOrAddr::Addr(ref addr) => Resolution::Addr(Some(*addr)),
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

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.name_or_addr {
            NameOrAddr::Name(ref name) => {
                write!(f, "{}:{}", name.host, name.port)
            }
            NameOrAddr::Addr(ref addr) => addr.fmt(f),
        }
    }
}
