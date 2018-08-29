use std::error::Error;
use std::fmt;
use std::net::SocketAddr;

use futures::{Async, Future, Poll, task};
use http::{self, uri};
use tower_service as tower;
use tower_h2;
use tower_reconnect::{Reconnect, Error as ReconnectError};

use ctx;
use endpoint::Endpoint;
use telemetry;
use transparency::{self, HttpBody, h1};
use transport;
use tls;
use watch_service::{WatchService, Rebind};

mod new_endpoint;
mod normalize_uri;
pub mod orig_proto;

pub use self::new_endpoint::NewEndpoint;
pub use self::normalize_uri::NormalizeUri;

/// Binds a `Service` from a `SocketAddr` for a pre-determined protocol.
pub struct BindProtocol<B> {
    new_endpoint: NewEndpoint<B>,
    protocol: Protocol,
}

/// A bound service that can re-bind itself on demand.
///
/// Reasons this would need to re-bind:
///
/// - `BindsPerRequest` can only service 1 request, and then needs to bind a
///   new service.
/// - If there is an error in the inner service (such as a connect error), we
///   need to throw it away and bind a new service.
pub struct BoundService<B>
where
    B: tower_h2::Body + Send + 'static,
    <B::Data as ::bytes::IntoBuf>::Buf: Send,
{
    new_endpoint: NewEndpoint<B>,
    binding: Binding<B>,
    /// Prevents logging repeated connect errors.
    ///
    /// Set back to false after a connect succeeds, to log about future errors.
    debounce_connect_error_log: bool,
    endpoint: Endpoint,
    protocol: Protocol,
}

/// Protocol portion of the `Recognize` key for a request.
///
/// This marks whether to use HTTP/2 or HTTP/1.x for a request. In
/// the case of HTTP/1.x requests, it also stores a "host" key to ensure
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
    Http2
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Authority(uri::Authority),
    NoAuthority,
}

pub struct RebindTls<B> {
    bind: NewEndpoint<B>,
    protocol: Protocol,
    endpoint: Endpoint,
}

pub type Service<B> = BoundService<B>;

pub type Stack<B> = WatchService<tls::ConditionalClientConfig, RebindTls<B>>;

type StackInner<B> = Reconnect<orig_proto::Upgrade<NormalizeUri<NewHttp<B>>>>;

pub type NewHttp<B> = telemetry::http::service::NewHttp<Client<B>, B, HttpBody>;

pub type HttpResponse = http::Response<telemetry::http::service::ResponseBody<HttpBody>>;

pub type HttpRequest<B> = http::Request<telemetry::http::service::RequestBody<B>>;

pub type Client<B> = transparency::Client<
    transport::metrics::Connect<transport::Connect>,
    ::logging::ClientExecutor<&'static str, SocketAddr>,
    B,
>;

#[derive(Copy, Clone, Debug)]
pub enum BufferSpawnError {
    Inbound,
    Outbound,
}

impl fmt::Display for BufferSpawnError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad(self.description())
    }
}

impl Error for BufferSpawnError {

    fn description(&self) -> &str {
        match *self {
            BufferSpawnError::Inbound =>
                "error spawning inbound buffer task",
            BufferSpawnError::Outbound =>
                "error spawning outbound buffer task",
        }
    }

    fn cause(&self) -> Option<&Error> { None }
}

// ===== impl Binding =====

impl<B> tower::Service for BoundService<B>
where
    B: tower_h2::Body + Send + 'static,
    <B::Data as ::bytes::IntoBuf>::Buf: Send,
{
    type Request = <Stack<B> as tower::Service>::Request;
    type Response = <Stack<B> as tower::Service>::Response;
    type Error = <Stack<B> as tower::Service>::Error;
    type Future = <Stack<B> as tower::Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        let ready = match self.binding {
            // A service is already bound, so poll its readiness.
            Binding::Bound(ref mut svc) |
            Binding::BindsPerRequest { next: Some(ref mut svc) } => {
                trace!("poll_ready: stack already bound");
                svc.poll_ready()
            }

            // If no stack has been bound, bind it now so that its readiness can be
            // checked. Store it so it can be consumed to dispatch the next request.
            Binding::BindsPerRequest { ref mut next } => {
                trace!("poll_ready: binding stack");
                let mut svc = self.bind.bind_stack(&self.endpoint, &self.protocol);
                let ready = svc.poll_ready();
                *next = Some(svc);
                ready
            }
        };

        // If there was a connect error, don't terminate this BoundService
        // completely. Instead, simply clean up the inner service, prepare to
        // make a new one, and tell our caller that we could maybe be ready
        // if they call `poll_ready` again.
        //
        // If they *don't* call `poll_ready` again, that's ok, we won't ever
        // try to connect again.
        match ready {
            Err(ReconnectError::Connect(err)) => {
                if !self.debounce_connect_error_log {
                    self.debounce_connect_error_log = true;
                    warn!("connect error to {:?}: {}", self.endpoint, err);
                } else {
                    debug!("connect error to {:?}: {}", self.endpoint, err);
                }
                match self.binding {
                    Binding::Bound(ref mut svc) => {
                        trace!("poll_ready: binding stack after error");
                        *svc = self.bind.bind_stack(&self.endpoint, &self.protocol);
                    },
                    Binding::BindsPerRequest { ref mut next } => {
                        trace!("poll_ready: dropping bound stack after error");
                        next.take();
                    }
                }

                // So, this service isn't "ready" yet. Instead of trying to make
                // it ready, schedule the task for notification so the caller can
                // determine whether readiness is still necessary (i.e. whether
                // there are still requests to be sent).
                //
                // But, to return NotReady, we must notify the task. So,
                // this notifies the task immediately, and figures that
                // whoever owns this service will call `poll_ready` if they
                // are still interested.
                task::current().notify();
                Ok(Async::NotReady)
            }
            // don't debounce on NotReady...
            Ok(Async::NotReady) => Ok(Async::NotReady),
            other => {
                trace!("poll_ready: ready for business");
                self.debounce_connect_error_log = false;
                other
            },
        }
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        match self.binding {
            Binding::Bound(ref mut svc) => svc.call(request),
            Binding::BindsPerRequest { ref mut next } => {
                // If a service has already been bound in `poll_ready`, consume it.
                // Otherwise, bind a new service on-the-spot.
                let bind = &self.bind;
                let endpoint = &self.endpoint;
                let protocol = &self.protocol;
                let mut svc = next.take()
                    .unwrap_or_else(|| {
                        bind.bind_stack(endpoint, protocol)
                    });
                svc.call(request)
            }
        }
    }
}

// ===== impl Protocol =====


impl Protocol {
    pub fn detect<B>(req: &http::Request<B>) -> Self {
        if req.version() == http::Version::HTTP_2 {
            return Protocol::Http2;
        }

        let was_absolute_form = h1::is_absolute_form(req.uri());
        trace!(
            "Protocol::detect(); req.uri='{:?}'; was_absolute_form={:?};",
            req.uri(), was_absolute_form
        );
        // If the request has an authority part, use that as the host part of
        // the key for an HTTP/1.x request.
        let host = Host::detect(req);

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
            &Protocol::Http1 { was_absolute_form, .. } => was_absolute_form,
            _ => false,
        }
    }

    pub fn can_reuse_clients(&self) -> bool {
        match *self {
            Protocol::Http2 | Protocol::Http1 { host: Host::Authority(_), .. } => true,
            _ => false,
        }
    }

    pub fn is_h1_upgrade(&self) -> bool {
        match *self {
            Protocol::Http1 { is_h1_upgrade: true, .. } => true,
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

impl Host {
    pub fn detect<B>(req: &http::Request<B>) -> Host {
        req
            .uri()
            .authority_part()
            .cloned()
            .or_else(|| h1::authority_from_host(req))
            .map(Host::Authority)
            .unwrap_or_else(|| Host::NoAuthority)
    }
}

// ===== impl RebindTls =====

impl<B> Rebind<tls::ConditionalClientConfig> for RebindTls<B>
where
    B: tower_h2::Body + Send + 'static,
    <B::Data as ::bytes::IntoBuf>::Buf: Send,
{
    type Service = StackInner<B>;

    fn rebind(&mut self, tls: &tls::ConditionalClientConfig) -> Self::Service {
        debug!(
            "rebinding endpoint stack for {:?}:{:?} on TLS config change",
            self.endpoint, self.protocol,
        );
        self.bind.bind_inner_stack(&self.endpoint, &self.protocol, tls)
    }
}
