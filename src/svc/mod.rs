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

use futures::future::{self, Future, FutureResult};
pub use tower_service::{NewService, Service};

pub mod and_then;
pub mod http;

pub trait NewClient {
    /// Describes how to build
    type Config;

    /// Indicates why the provided `Target` cannot be used to instantiate a client.
    type Error;

    /// Serves requests on behalf of a config.
    type Client: Service;

    type Future: Future<Item = Self::Client, Error = Self::Error>;

    /// If the provided `Config` is valid, immediately return a `Service` that may
    /// become ready lazily, i.e. as the config is resolved and connections are
    /// established.
    fn new_client(&self, t: &Self::Config) -> Self::Future;

    fn into_new_service(self, config: Self::Config) -> IntoNewService<Self>
    where
        Self: Sized,
    {
        IntoNewService {
            new_client: self,
            config,
        }
    }
}

/// Composes client functionality for `NewClient`
pub trait Stack<Next: NewClient> {
    type Config;
    type Error;
    type Client: Service;
    type NewClient: NewClient<
        Config = Self::Config,
        Error = Self::Error,
        Client = Self::Client,
    >;

    fn build(&self, next: Next) -> Self::NewClient;

    fn and_then<M, N>(self, inner: M) -> and_then::AndThen<Self, M, N>
    where
        Self: Sized + Stack<M::NewClient>,
        M: Stack<N>,
        N: NewClient,
    {
        and_then::AndThen::new(self, inner)
    }
}

#[derive(Clone, Debug)]
pub struct IntoNewService<N: NewClient> {
    new_client: N,
    config: N::Config,
}

impl<N: NewClient> IntoNewService<N> {
    fn config(&self) -> &N::Config {
        &self.config
    }
}

impl<N: NewClient> NewService for IntoNewService<N> {
    type Request = <N::Service as Service>::Request;
    type Response = <N::Service as Service>::Response;
    type Error = <N::Service as Service>::Error;
    type Service = N::Service;
    type InitError = N::Error;
    type Future = FutureResult<Self::Service, Self::InitError>;

    fn new_service(&self) -> Self::Future {
        future::result(self.new_client.new_client(&self.config))
    }
}
