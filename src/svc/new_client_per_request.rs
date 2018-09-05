use futures::Poll;
use std::fmt;

use super::{Stack, MakeService, Service, ValidMakeService};

pub struct Mod;

/// A `MakeService` that builds a single-serving client for each request.
#[derive(Clone, Debug)]
pub struct Make<N: MakeService + Clone>(N);

/// A `Service` that optionally uses a
///
/// `ClientPerRequest` does not handle any underlying errors and it is expected that an
/// instance will not be used after an error is returned.
pub struct ClientPerRequest<N: MakeService> {
    // When `poll_ready` is called, the _next_ service to be used may be bound
    // ahead-of-time. This stack is used only to serve the next request to this
    // service.
    next: Option<N::Service>,
    make_service: ValidMakeService<N>,
}

// ==== MakeServicePerRequest====

impl<N> Stack<N> for Mod
where
    N: MakeService + Clone,
    N::Config: Clone,
    N::Error: fmt::Debug,
{
    type Config = <Make<N> as MakeService>::Config;
    type Error = <Make<N> as MakeService>::Error;
    type Service = <Make<N> as MakeService>::Service;
    type MakeService = Make<N>;

    fn build(&self, next: N) -> Self::MakeService {
        Make(next)
    }
}

impl<N> MakeService for Make<N>
where
    N: MakeService + Clone,
    N::Config: Clone,
    N::Error: fmt::Debug,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = ClientPerRequest<N>;

    fn make_service(&self, config: &N::Config) -> Result<Self::Service, N::Error> {
        let (next, valid) = ValidMakeService::try(&self.0, config)?;
        Ok(ClientPerRequest {
            next: Some(next),
            make_service: valid,
        })
    }
}

// ==== ClientPerRequest ====

impl<N> Service for ClientPerRequest<N>
where
    N: MakeService + Clone,
    N::Config: Clone,
    N::Error: fmt::Debug,
{
    type Request = <<N as MakeService>::Service as Service>::Request;
    type Response = <<N as MakeService>::Service as Service>::Response;
    type Error = <<N as MakeService>::Service as Service>::Error;
    type Future = <<N as MakeService>::Service as Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        if let Some(ref mut svc) = self.next {
            return svc.poll_ready();
        }

        trace!("poll_ready: new disposable client");
        let mut svc = self.make_service.make_service();
        let ready = svc.poll_ready()?;
        self.next = Some(svc);
        Ok(ready)
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        // If a service has already been bound in `poll_ready`, consume it.
        // Otherwise, bind a new service on-the-spot.
        self.next
            .take()
            .unwrap_or_else(|| self.make_service.make_service())
            .call(request)
    }
}
