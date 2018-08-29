use super::{MakeClient, NewClient, Service};

pub struct Make;

/// A `NewClient` that builds a single-serving client for each request.
#[derive(Clone, Debug)]
pub struct NewClientPerRequest<N: NewClient>(N);

/// A `Service` that optionally uses a
///
/// `ClientPerRequest` does not handle any underlying errors and it is expected that an
/// instance will not be used after an error is returned.
#[derive(Clone, Debug)]
pub struct ClientPerRequest<N: NewClient> {
    // When `poll_ready` is called, the _next_ service to be used may be bound
    // ahead-of-time. This stack is used only to serve the next request to this
    // service.
    next: Option<N::Client>,
    new_client: ValidNewClient<N>,
}

/// A `NewClient` and target that infallibly build services.
#[derive(Clone, Debug)]
struct ValidNewClient<N: NewClient> {
    new_client: N,
    target: N::Target,
}

// ==== ValidNewClient ====

impl<N: NewClient> ValidNewClient<N> {
    fn mk(&mut self) -> N::Client {
        self.new_client
            .new_client(&self.target)
            .expect("target must be valid")
    }
}

// ==== NewClientPerRequest====

impl<N: NewClient> MakeClient<N> for Make {
    type NewClient = NewClientPerRequest<N>;

    fn make_client(&self, next: N) -> Self::NewClient {
        NewClientPerRequest(next)
    }
}

impl<N: NewClient + Clone> NewClient for NewClientPerRequest<N> {
    type Target = N::Target;
    type Error = N::Error;
    type Client = ClientPerRequest<N>;

    pub fn new_client(&mut self, target: N::Target) -> Result<Self, N::Error> {
        let next = self.0.new_client(&target)?;
        let valid = ValidNewClient(self.0.clone(), target);
        Ok(ClientPerRequest {
            next: Some(next),
            new_client: valid,
        })
    }
}

// ==== ClientPerRequest ====

impl<N: NewClient> Service for ClientPerRequest<N> {
    type Request = <<N as NewClient>::Service as Service>::Request;
    type Response = <<N as NewClient>::Service as Service>::Response;
    type Error = <<N as NewClient>::Service as Service>::Error;
    type Future = <<N as NewClient>::Service as Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        if let Some(ref mut svc) = self.next {
            return svc.poll_ready();
        }

        trace!("poll_ready: new disposable client");
        let mut svc = self.new_client.mk();
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
