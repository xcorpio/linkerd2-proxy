//! Reponsible for proxying traffic from a server interface.
//!
//! As the `Server` is invoked with transports, it may terminate a TLS session
//! and determine the peer's identity and determine whether the connection is
//! transporting HTTP. If the transport does not contain HTTP traffic, then the
//! TCP stream is blindly forwarded (according to the original socket's
//! `SO_ORIGINAL_DST` option). Otherwise, an HTTP service established for the
//! connection through which requests are dispatched.
//!
//! Once a request is routed, the `Client` type can be used to establish a
//! `Service` that hides the type differences between HTTP/1 and HTTP/2 clients.
//!
//! This module is intended only to store the infrastructure for building a
//! proxy. The specific logic implemented by a proxy should live elsewhere.

use std::collections::HashMap;
use std::net::SocketAddr;
use transport::{DnsNameAndPort, tls};
use Conditional;

pub mod http;
mod protocol;
mod reconnect;
mod server;
mod tcp;

pub use self::reconnect::Reconnect;
pub use self::server::Server;

pub struct Source {
    remote: SocketAddr,
    local: SocketAddr,
    orig_dst: Option<SocketAddr>,
    _p: (),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Destination {
    /// A logical, lazily-bound endpoint.
    Name(DnsNameAndPort),

    /// A single, bound endpoint.
    Addr(SocketAddr),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    address: SocketAddr,
    labels: HashMap<String, String>,
    h2_prior_knowledge: ProtocolHint,
    tls_identity: Conditional<tls::Identity, tls::ReasonForNoIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolHint {
    /// We don't what the destination understands, so forward messages in the
    /// protocol we received them in.
    Unknown,
    /// The destination can receive HTTP2 messages.
    Http2,
}

pub struct Client {
    address: SocketAddr,
    labels: HashMap<String, String>

}
