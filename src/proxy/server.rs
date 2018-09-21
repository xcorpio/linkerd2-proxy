use futures::{future::{self, Either}, Future};
use h2;
use http;
use hyper;
use indexmap::IndexSet;
use std::error;
use std::net::SocketAddr;
use tokio_connect::Connect;
use tokio::io::{AsyncRead, AsyncWrite};
use tower_h2;

use drain;
use svc::{Make, Service, stack::MakeNewService};
use transport::{self, tls, Connection, GetOriginalDst, Peek};
use proxy::http::glue::{HttpBody, HttpBodyNewSvc, HyperServerSvc};
use proxy::protocol::Protocol;
use proxy::tcp;
use super::Accept;

#[derive(Clone, Debug)]
pub struct Source {
    pub remote: SocketAddr,
    pub local: SocketAddr,
    pub orig_dst: Option<SocketAddr>,
    pub tls_status: tls::Status,
    _p: (),
}

/// A protocol-transparent Server!
///
/// This type can `serve` new connections, determine what protocol
/// the connection is speaking, and route it to the corresponding
/// service.
pub struct Server<A, C, R, B, G>
where
    A: Make<Source, Error = ()> + Clone,
    A::Value: Accept<Connection>,
    C: Make<SocketAddr, Error = ()> + Clone,
    C::Value: Connect,
    R: Make<Source, Error = ()> + Clone,
    R::Value: Service<
        Request = http::Request<HttpBody>,
        Response = http::Response<B>,
    >,
    B: tower_h2::Body,
    G: GetOriginalDst,
{
    disable_protocol_detection_ports: IndexSet<u16>,
    drain_signal: drain::Watch,
    get_orig_dst: G,
    h1: hyper::server::conn::Http,
    h2_settings: h2::server::Builder,
    listen_addr: SocketAddr,
    accept: A,
    connect: C,
    route: R,
    log: ::logging::Server,
}

impl<A, C, R, B, G> Server<A, C, R, B, G>
where
    A: Make<Source, Error = ()> + Clone,
    A::Value: Accept<Connection>,
    <A::Value as Accept<Connection>>::Io: Peek,
    C: Make<SocketAddr, Error = ()> + Clone,
    C::Value: Connect,
    R: Make<Source, Error = ()> + Clone,
    R::Value: Service<
        Request = http::Request<HttpBody>,
        Response = http::Response<B>,
    >,
    <R::Value as Service>::Error: error::Error + Send + Sync + 'static,
    <R::Value as Service>::Future: Send + 'static,
    B: tower_h2::Body + Default + Send + 'static,
    B::Data: Send,
    <B::Data as ::bytes::IntoBuf>::Buf: Send,
    G: GetOriginalDst,
{

   /// Creates a new `Server`.
    pub fn new(
        proxy_ctx: ::ctx::Proxy,
        listen_addr: SocketAddr,
        get_orig_dst: G,
        accept: A,
        connect: C,
        route: R,
        disable_protocol_detection_ports: IndexSet<u16>,
        drain_signal: drain::Watch,
        h2_settings: h2::server::Builder,
    ) -> Self {
        let log = ::logging::Server::proxy(proxy_ctx, listen_addr);
        Server {
            disable_protocol_detection_ports,
            drain_signal,
            get_orig_dst,
            h1: hyper::server::conn::Http::new(),
            h2_settings,
            listen_addr,
            accept,
            connect,
            route,
            log,
        }
    }

    pub fn log(&self) -> &::logging::Server {
        &self.log
    }

    /// Handle a new connection.
    ///
    /// This will peek on the connection for the first bytes to determine
    /// what protocol the connection is speaking. From there, the connection
    /// will be mapped into respective services, and spawned into an
    /// executor.
    pub fn serve(&self, connection: Connection, remote_addr: SocketAddr)
        -> impl Future<Item=(), Error=()>
    {
        let orig_dst = connection.original_dst_addr(&self.get_orig_dst);

        let log = self.log.clone()
            .with_remote(remote_addr);

        let source = Source {
            remote: remote_addr,
            local: connection.local_addr().unwrap_or(self.listen_addr),
            orig_dst,
            tls_status: connection.tls_status(),
            _p: (),
        };

        let io = self.accept.make(&source)
            .expect("source must be acceptable")
            .accept(connection);

        // We are using the port from the connection's SO_ORIGINAL_DST to
        // determine whether to skip protocol detection, not any port that
        // would be found after doing discovery.
        let disable_protocol_detection = orig_dst
            .map(|addr| {
                self.disable_protocol_detection_ports.contains(&addr.port())
            })
            .unwrap_or(false);

        if disable_protocol_detection {
            trace!("protocol detection disabled for {:?}", orig_dst);
            let fut = forward_tcp(
                io,
                self.connect.clone(),
                &source,
                self.drain_signal.clone(),
            );

            return log.future(Either::B(fut));
        }

        let detect_protocol = io.peek()
            .map_err(|e| debug!("peek error: {}", e))
            .map(|io| {
                let p = Protocol::detect(io.peeked());
                (p, io)
            });

        let h1 = self.h1.clone();
        let h2_settings = self.h2_settings.clone();
        let route = self.route.clone();
        let connect = self.connect.clone();
        let drain_signal = self.drain_signal.clone();
        let log_clone = log.clone();
        let serve = detect_protocol
            .and_then(move |(proto, io)| match proto {
                None => Either::A({
                    trace!("did not detect protocol; forwarding TCP");
                    forward_tcp(io, connect, &source, drain_signal)
                }),

                Some(proto) => Either::B(match proto {
                    Protocol::Http1 => Either::A({
                        trace!("detected HTTP/1");
                        match route.make(&source) {
                            Err(()) => Either::A({
                                error!("failed to build HTTP/1 client");
                                future::err(())
                            }),
                            Ok(s) => Either::B({
                                let svc = HyperServerSvc::new(
                                    s,
                                    drain_signal.clone(),
                                    log_clone.executor(),
                                );
                                // Enable support for HTTP upgrades (CONNECT and websockets).
                                let conn = h1
                                    .serve_connection(io, svc)
                                    .with_upgrades();
                                drain_signal
                                    .watch(conn, |conn| {
                                        conn.graceful_shutdown();
                                    })
                                    .map(|_| ())
                                    .map_err(|e| trace!("http1 server error: {:?}", e))
                            }),
                        }
                    }),
                    Protocol::Http2 => Either::B({
                        trace!("detected HTTP/2");
                        let new_service = MakeNewService::new(route, source.clone());
                        let h2 = tower_h2::Server::new(
                            HttpBodyNewSvc::new(new_service),
                            h2_settings,
                            log_clone.executor(),
                        );
                        let serve = h2.serve_modified(io, move |r: &mut http::Request<()>| {
                            r.extensions_mut().insert(source.clone());
                        });
                        drain_signal
                            .watch(serve, |conn| conn.graceful_shutdown())
                            .map_err(|e| trace!("h2 server error: {:?}", e))
                    }),
                }),
            });

        log.future(Either::A(serve))
    }
}

fn forward_tcp<I, C, T>(
    io: I,
    connect: C,
    target: &T,
    drain_signal: drain::Watch,
) -> impl Future<Item=(), Error=()> + Send + 'static
where
    I: AsyncRead + AsyncWrite + Send + 'static,
    C: Make<T, Error = ()>,
    C::Value: Connect,
    <C::Value as Connect>::Connected: Send + 'static,
{
    let connect = match connect.make(&target) {
        Ok(c) => c,
        Err(()) => {
            error!("tcp forward error: {:?}", source);
            return Either::A(future::ok(()));
        }
    };

    let fwd = connect.connect()
        .and_then(move |client| tcp::Duplex::new(io, client))
        .map_err(|e| error!("tcp forward duplex error: {}", e));

    // There's nothing to do when drain is signaled, we just have to hope
    // the sockets finish soon. However, the drain signal still needs to
    // 'watch' the TCP future so that the process doesn't close early.
    Either::B(drain_signal.watch(fwd, |_| ()))
}
