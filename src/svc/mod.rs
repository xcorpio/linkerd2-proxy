#![allow(dead_code)]

//! Infrastructure for proxying request-response message streams
//!
//! This module contains utilities for proxying request-response streams. This
//! module borrows (and re-exports) from `tower`.
//!
//! ## Clients
//!
//! A client is a `Service` through which the proxy may dispatch requests.
//!
//! In the proxy, there are currently two types of clients:
//!
//! - As the proxy routes requests to an outbound `Destination`, a client
//!   service is resolves the destination to and load balances requests
//!   over its endpoints.
//!
//! - As an outbound load balancer dispatches a request to an endpoint, or as
//!   the inbound proxy fowards an inbound request, a client service models an
//!   individual `SocketAddr`.

use futures::future::{self, FutureResult};
pub use tower_service::{NewService, Service};

pub mod and_then;
pub mod either;
pub mod http;
pub mod make_per_request;
pub mod optional;
pub mod reconnect;
mod valid;

use self::valid::ValidMakeService;

pub trait MakeService {
    /// Describes how to build
    type Config;

    /// Indicates why the provided `Target` cannot be used to instantiate a client.
    type Error;

    /// Serves requests on behalf of a config.
    type Service: Service;

    /// If the provided `Config` is valid, immediately return a `Service` that may
    /// become ready lazily, i.e. as the config is resolved and connections are
    /// established.
    fn make_service(&self, t: &Self::Config) -> Result<Self::Service, Self::Error>;

    fn into_new_service(self, config: Self::Config) -> IntoNewService<Self>
    where
        Self: Sized,
    {
        IntoNewService {
            make_service: self,
            config,
        }
    }
}

/// Composes client functionality for `MakeService`
pub trait Stack<Next: MakeService> {
    type Config;
    type Error;
    type Service: Service;
    type MakeService: MakeService<
        Config = Self::Config,
        Error = Self::Error,
        Service = Self::Service,
    >;

    fn build(&self, next: Next) -> Self::MakeService;

    fn and_then<M, N>(self, inner: M) -> and_then::AndThen<Self, M, N>
    where
        Self: Sized + Stack<M::MakeService>,
        M: Stack<N>,
        N: MakeService,
    {
        and_then::AndThen::new(self, inner)
    }
}

#[derive(Clone, Debug)]
pub struct IntoNewService<N: MakeService> {
    make_service: N,
    config: N::Config,
}

impl<N: MakeService> IntoNewService<N> {
    fn config(&self) -> &N::Config {
        &self.config
    }
}

impl<N: MakeService> NewService for IntoNewService<N> {
    type Request = <N::Service as Service>::Request;
    type Response = <N::Service as Service>::Response;
    type Error = <N::Service as Service>::Error;
    type Service = N::Service;
    type InitError = N::Error;
    type Future = FutureResult<Self::Service, Self::InitError>;

    fn new_service(&self) -> Self::Future {
        future::result(self.make_service.make_service(&self.config))
    }
}
