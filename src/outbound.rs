use futures::{Async, Poll};
use http;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::fmt;

use control::destination::{Metadata, ProtocolHint};
use proxy::{
    self,
    http::{client, h1, orig_proto, router, Settings},
    resolve,
};
use svc;
use transport::{connect, tls, DnsNameAndPort, Host, HostAndPort};
use Conditional;

#[derive(Clone, Debug, Default)]
pub struct Recognize {}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Destination {
    pub name_or_addr: NameOrAddr,
    pub settings: Settings,
    _p: (),
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub connect: connect::Target,
    pub settings: Settings,
    pub metadata: Metadata,
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
    Name(R, Settings),
    Addr(Option<SocketAddr>, Settings),
}

impl<B> router::Recognize<http::Request<B>> for Recognize {
    type Target = Destination;

    // Route the request by its destination AND PROTOCOL. This prevents HTTP/1
    // requests from being routed to HTTP/2 servers, and vice versa.
    fn recognize(&self, req: &http::Request<B>) -> Option<Self::Target> {
        let name_or_addr = Self::name_or_addr(req)?;
        let settings = Settings::detect(req);
        Some(Destination {
            name_or_addr,
            settings,
            _p: (),
        })
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
            .or_else(|| h1::authority_from_host(req).and_then(|h| Self::normalize(&h)))
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
            Some(HostAndPort {
                host: Host::DnsName(host),
                port,
            }) => {
                let name_or_addr = DnsNameAndPort { host, port };
                Some(NameOrAddr::Name(name_or_addr))
            }

            Some(HostAndPort {
                host: Host::Ip(ip),
                port,
            }) => {
                let name_or_addr = SocketAddr::from((ip, port));
                Some(NameOrAddr::Addr(name_or_addr))
            }

            None => req
                .extensions()
                .get::<proxy::server::Source>()
                .and_then(|src| src.orig_dst_if_not_local())
                .map(NameOrAddr::Addr),
        }
    }
}

impl<R> Resolve<R>
where
    R: resolve::Resolve<DnsNameAndPort, Endpoint = Metadata>,
{
    pub fn new(resolve: R) -> Self {
        Resolve(resolve)
    }
}

impl<R> resolve::Resolve<Destination> for Resolve<R>
where
    R: resolve::Resolve<DnsNameAndPort, Endpoint = Metadata>,
{
    type Endpoint = Endpoint;
    type Resolution = Resolution<R::Resolution>;

    fn resolve(&self, t: &Destination) -> Self::Resolution {
        match t.name_or_addr {
            NameOrAddr::Name(ref name) =>
                Resolution::Name(self.0.resolve(&name), t.settings.clone()),
            NameOrAddr::Addr(ref addr) =>
                Resolution::Addr(Some(*addr), t.settings.clone()),
        }
    }
}

impl<R> resolve::Resolution for Resolution<R>
where
    R: resolve::Resolution<Endpoint = Metadata>,
{
    type Endpoint = Endpoint;
    type Error = R::Error;

    fn poll(&mut self) -> Poll<resolve::Update<Self::Endpoint>, Self::Error> {
        match self {
            Resolution::Name(ref mut res, ref settings) => match try_ready!(res.poll()) {
                resolve::Update::Remove(addr) =>
                    Ok(Async::Ready(resolve::Update::Remove(addr))),
                resolve::Update::Make(addr, metadata) => {
                    // If the endpoint does not have TLS, notethe reason.
                    // Otherwise, indicate that we don't (yet have a TLS
                    // config). This value may be changed by a stack layer
                    // that provides TLS configuration.
                    let tls = match metadata.tls_identity() {
                        Conditional::None(reason) => reason.into(),
                        Conditional::Some(_) => tls::ReasonForNoTls::NoConfig,
                    };
                    let ep = Endpoint {
                        connect: connect::Target::new(addr, Conditional::None(tls)),
                        settings: settings.clone(),
                        metadata,
                        _p: (),
                    };
                    Ok(Async::Ready(resolve::Update::Make(addr, ep)))
                }
            }
            Resolution::Addr(ref mut addr, ref settings) => match addr.take() {
                Some(addr) => {
                    let tls = tls::ReasonForNoIdentity::NoAuthorityInHttpRequest;
                    let ep = Endpoint {
                        connect: connect::Target::new(addr, Conditional::None(tls.into())),
                        settings: settings.clone(),
                        metadata: Metadata::none(tls),
                        _p: (),
                    };
                    let up = resolve::Update::Make(addr, ep);
                    Ok(Async::Ready(up))
                }
                None => Ok(Async::NotReady),
            },
        }
    }
}

impl fmt::Display for Destination {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.name_or_addr {
            NameOrAddr::Name(ref name) => write!(f, "{}:{}", name.host, name.port),
            NameOrAddr::Addr(ref addr) => addr.fmt(f),
        }
    }
}

pub fn orig_proto_upgrade<M>() -> LayerUpgrade<M> {
    LayerUpgrade(PhantomData)
}

#[derive(Debug)]
pub struct LayerUpgrade<M>(PhantomData<fn() -> (M)>);

#[derive(Clone, Debug)]
pub struct MakeUpgrade<M>
where
    M: svc::Make<Endpoint>,
{
    inner: M,
}

impl<M> Clone for LayerUpgrade<M> {
    fn clone(&self) -> Self {
        LayerUpgrade(PhantomData)
    }
}

impl<M, A, B> svc::Layer<Endpoint, Endpoint, M> for LayerUpgrade<M>
where
    M: svc::Make<Endpoint>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
{
    type Value = <MakeUpgrade<M> as svc::Make<Endpoint>>::Value;
    type Error = <MakeUpgrade<M> as svc::Make<Endpoint>>::Error;
    type Make = MakeUpgrade<M>;

    fn bind(&self, inner: M) -> Self::Make {
        MakeUpgrade {
            inner,
        }
    }
}

impl<M, A, B> svc::Make<Endpoint> for MakeUpgrade<M>
where
    M: svc::Make<Endpoint>,
    M::Value: svc::Service<Request = http::Request<A>, Response = http::Response<B>>,
{
    type Value = orig_proto::Upgrade<M::Value>;
    type Error = M::Error;

    fn make(&self, endpoint: &Endpoint) -> Result<Self::Value, Self::Error> {
        let mut endpoint = endpoint.clone();
        endpoint.settings = Settings::Http2;

        let inner = self.inner.make(&endpoint)?;
        Ok(inner.into())
    }
}

impl Endpoint {
    pub fn can_use_orig_proto(&self) -> bool {
        match self.metadata.protocol_hint() {
            ProtocolHint::Unknown => false,
            ProtocolHint::Http2 => true,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.connect.addr.fmt(f)
    }
}

impl From<Endpoint> for client::Config {
    fn from(ep: Endpoint) -> Self {
        client::Config::new(ep.connect, ep.settings)
    }
}

