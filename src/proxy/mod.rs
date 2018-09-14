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

use tokio::io::{AsyncRead, AsyncWrite};

pub mod buffer;
pub mod http;
pub mod limit;
mod protocol;
mod reconnect;
pub mod resolve;
pub mod server;
mod tcp;
pub mod timeout;

pub use self::reconnect::Reconnect;
pub use self::resolve::{Resolve, Resolution};
pub use self::server::{Server, Source};

pub trait Accept<T: AsyncRead + AsyncWrite> {
    type Io: AsyncRead + AsyncWrite;

    fn accept(&self, inner: T) -> Self::Io;
}

impl<T> Accept<T> for ()
where
    T: AsyncRead + AsyncWrite,
{
    type Io = T;

    #[inline]
    fn accept(&self, inner: T) -> T {
        inner
    }
}
