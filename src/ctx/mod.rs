use http::{self, uri};
use svc::http::h1;

pub mod transport;

/// Indicates the orientation of traffic, relative to a sidecar proxy.
///
/// Each process exposes two proxies:
/// - The _inbound_ proxy receives traffic from another services forwards it to within the
///   local instance.
/// - The  _outbound_ proxy receives traffic from the local instance and forwards it to a
///   remote service.
///
/// This type is used for the purposes of caching and telemetry.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Proxy {
    Inbound,
    Outbound,
}

/// Marks whether to use HTTP/2 or HTTP/1.x for a request.
///
/// In the case of HTTP/1.x requests, it also stores a "host" key to ensure
/// that each host receives its own connection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Http1 {
        host: Host,

        /// Whether the request wants to use HTTP/1.1's Upgrade mechanism.
        ///
        /// Since these cannot be translated into orig-proto, it must be
        /// tracked here so as to allow those with `is_h1_upgrade: true` to
        /// use an explicitly HTTP/1 service, instead of one that might
        /// utilize orig-proto.
        is_h1_upgrade: bool,

        /// Whether or not the request URI was in absolute form.
        ///
        /// This is used to configure Hyper's behaviour at the connection
        /// level, so it's necessary that requests with and without
        /// absolute URIs be bound to separate service stacks. It is also
        /// used to determine what URI normalization will be necessary.
        was_absolute_form: bool,
    },
    Http2,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Authority(uri::Authority),
    NoAuthority,
}

impl Proxy {
    pub fn as_str(&self) -> &'static str {
        match *self {
            Proxy::Inbound => "in",
            Proxy::Outbound => "out",
        }
    }
}

// ==== Host ====

impl Host {
    fn from_request<B>(req: &http::Request<B>) -> Self {
        req.uri()
            .authority_part()
            .cloned()
            .or_else(|| h1::authority_from_host(req))
            .map(Host::Authority)
            .unwrap_or_else(|| Host::NoAuthority)
    }
}

// ===== impl Protocol =====

impl Protocol {
    fn from_request<B>(req: &http::Request<B>) -> Self {
        if req.version() == http::Version::HTTP_2 {
            return Protocol::Http2;
        }

        let was_absolute_form = h1::is_absolute_form(req.uri());
        trace!(
            "Protocol::detect(); req.uri='{:?}'; was_absolute_form={:?};",
            req.uri(),
            was_absolute_form
        );
        // If the request has an authority part, use that as the host part of
        // the key for an HTTP/1.x request.
        let host = Host::from_request(req);

        let is_h1_upgrade = h1::wants_upgrade(req);

        Protocol::Http1 {
            host,
            is_h1_upgrade,
            was_absolute_form,
        }
    }

    /// Returns true if the request was originally received in absolute form.
    pub fn was_absolute_form(&self) -> bool {
        match self {
            &Protocol::Http1 {
                was_absolute_form, ..
            } => was_absolute_form,
            _ => false,
        }
    }

    pub fn can_reuse_clients(&self) -> bool {
        match *self {
            Protocol::Http2
            | Protocol::Http1 {
                host: Host::Authority(_),
                ..
            } => true,
            _ => false,
        }
    }

    pub fn is_h1_upgrade(&self) -> bool {
        match *self {
            Protocol::Http1 {
                is_h1_upgrade: true,
                ..
            } => true,
            _ => false,
        }
    }

    pub fn is_http2(&self) -> bool {
        match *self {
            Protocol::Http2 => true,
            _ => false,
        }
    }
}

#[cfg(test)]
pub mod test_util {
    use indexmap::IndexMap;
    use std::{
        net::SocketAddr,
        sync::Arc,
    };
    use super::{Proxy, transport};

    use control::destination;
    use tls;
    use conditional::Conditional;

    fn addr() -> SocketAddr {
        ([1, 2, 3, 4], 5678).into()
    }

    pub fn server(
        proxy: Proxy,
        tls: transport::TlsStatus
    ) -> Arc<transport::Server> {
        transport::Server::new(proxy, &addr(), &addr(), &Some(addr()), tls)
    }

    pub fn client(
        proxy: Proxy,
        labels: IndexMap<String, String>,
        tls: transport::TlsStatus,
    ) -> Arc<transport::Client> {
        let meta = destination::Metadata::new(
            labels,
            destination::ProtocolHint::Unknown,
            Conditional::None(tls::ReasonForNoIdentity::NotProvidedByServiceDiscovery)
        );
        transport::Client::new(proxy, &addr(), meta, tls)
    }
}
