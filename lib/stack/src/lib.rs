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
//!
//! ## TODO
//!
//! * Move HTTP-specific service infrastructure into `svc::http`.

extern crate futures;
#[macro_use]
extern crate log;
extern crate tower_service as svc;

pub mod either;
pub mod make_new_service;
pub mod make_per_request;
pub mod optional;
pub mod layer;
pub mod watch;
pub mod when;

pub use self::either::Either;
pub use self::optional::Optional;
pub use self::layer::Layer;
pub use self::make_new_service::MakeNewService;
//pub use self::watch::Watch;
//pub use self::when::When;

pub trait Make<Target> {
    type Value;
    type Error;

    fn make(&self, t: &Target) -> Result<Self::Value, Self::Error>;

    fn push<U, L>(self, layer: L) -> L::Make
    where
        L: Layer<U, Target, Self>,
        Self: Sized,
    {
        layer.bind(self)
    }
}
