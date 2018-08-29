use super::NewClient;

#[derive(Debug)]
pub struct NewClientPerRequest<N: NewClient>(Inner<N>);

#[derive(Debug)]
pub enum Inner<N: NewClient> {
    Reusable(N::Client),
    Disposable {
        // When `poll_ready` is called, the _next_ service to be used may be bound
        // ahead-of-time. This stack is used only to serve the next request to this
        // service.
        next: Option<N::Client>,
        target: N::Target,
        new_client: N,
    },
}

impl<N: NewClient> NewClientPerRequest {
    pub fn reusable(svc: N::Client) -> Self {
        NewClientPerRequest(Inner::Reusable(svc))
    }

    pub fn disposable(mut new_client: N, target: N::Target) -> Result<Self, N::Error> {
        let next = new_client.new_client(&target)?;

        Ok(NewClientPerRequest(Inner::Disposable {
            next: Some(next),
            new_client,
            target,
        }))
    }
}

impl<N: NewClient> tower::Service for NewClientPerRequest<N> {
    type Request = <<N as NewClient>::Service as Service>::Request;
    type Response = <<N as NewClient>::Service as Service>::Response;
    type Error = <<N as NewClient>::Service as Service>::Error;
    type Future = <<N as NewClient>::Service as Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        let ready = match self.0 {
            // A service is already bound, so poll its readiness.
            Inner::Reusable(ref mut svc) |
            Inner::Displosable { next: Some(ref mut svc), .. } => {
                trace!("poll_ready: reusing client");
                svc.poll_ready()
            }

            // If no stack has been bound, bind it now so that its readiness can be
            // checked. Store it so it can be consumed to dispatch the next request.
            Inner::Displosable { ref mut next, ref mut new_client, ref target } => {
                trace!("poll_ready: new disposable client");

                let mut svc = new_client.new_client(&target)
                    .expect("invalid target should be caught by constructor");

                let ready = svc.poll_ready();
                *next = Some(svc);
                ready
            }
        };

        if ready.is_err() {
            if let Inner::Disposable { ref mut next, .. } = self.0 {
                drop(next.take());
            }
        }

        ready
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        match self.0 {
            Inner::Reusable(ref mut svc) => svc.call(request),
            Inner::Displosable { ref mut next } => {
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
