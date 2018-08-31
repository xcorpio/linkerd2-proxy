use futures::Poll;
use std::fmt;

use super::{MakeClient, NewClient, Service, ValidNewClient};

pub struct Make;

/// A `NewClient` that builds a single-serving client for each request.
#[derive(Clone, Debug)]
pub struct NewClientPerRequest<N: NewClient + Clone>(N);

/// A `Service` that optionally uses a
///
/// `ClientPerRequest` does not handle any underlying errors and it is expected that an
/// instance will not be used after an error is returned.
pub struct ClientPerRequest<N: NewClient> {
    // When `poll_ready` is called, the _next_ service to be used may be bound
    // ahead-of-time. This stack is used only to serve the next request to this
    // service.
    next: Option<N::Client>,
    new_client: ValidNewClient<N>,
}

// ==== NewClientPerRequest====

impl<N> MakeClient<N> for Make
where
    N: NewClient + Clone,
    N::Target: Clone,
    N::Error: fmt::Debug,
{
    type Target = <NewClientPerRequest<N> as NewClient>::Target;
    type Error = <NewClientPerRequest<N> as NewClient>::Error;
    type Client = <NewClientPerRequest<N> as NewClient>::Client;
    type NewClient = NewClientPerRequest<N>;

    fn make_client(&self, next: N) -> Self::NewClient {
        NewClientPerRequest(next)
    }
}

impl<N> NewClient for NewClientPerRequest<N>
where
    N: NewClient + Clone,
    N::Target: Clone,
    N::Error: fmt::Debug,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = ClientPerRequest<N>;

    fn new_client(&self, target: &N::Target) -> Result<Self::Client, N::Error> {
        let next = self.0.new_client(&target)?;
        let valid = ValidNewClient {
            new_client: self.0.clone(),
            target: target.clone(),
        };
        Ok(ClientPerRequest {
            next: Some(next),
            new_client: valid,
        })
    }
}

// ==== ClientPerRequest ====

impl<N> Service for ClientPerRequest<N>
where
    N: NewClient + Clone,
    N::Target: Clone,
    N::Error: fmt::Debug,
{
    type Request = <<N as NewClient>::Client as Service>::Request;
    type Response = <<N as NewClient>::Client as Service>::Response;
    type Error = <<N as NewClient>::Client as Service>::Error;
    type Future = <<N as NewClient>::Client as Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        if let Some(ref mut svc) = self.next {
            return svc.poll_ready();
        }

        trace!("poll_ready: new disposable client");
        let mut svc = self.new_client.new_client();
        let ready = svc.poll_ready()?;
        self.next = Some(svc);
        Ok(ready)
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        // If a service has already been bound in `poll_ready`, consume it.
        // Otherwise, bind a new service on-the-spot.
        self.next.take()
            .unwrap_or_else(|| self.new_client.new_client())
            .call(request)
    }
}
