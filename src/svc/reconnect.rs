use std::fmt;

use futures::{Async, Poll, task};
use tower_reconnect;

use super::{NewClient, Service, NewService, IntoNewService};

pub struct Reconnect<N: NewClient>(N);

pub struct ReconnectService<N: NewService + Clone> {
    new_service: N,
    inner: tower_reconnect::Reconnect<N>,

    /// Prevents logging repeated connect errors.
    ///
    /// Set back to false after a connect succeeds, to log about future errors.
    mute_connect_error_log: bool,
}

// ===== impl Reconnect =====

impl<N> NewClient for Reconnect<N>
where
    N: NewClient + Clone,
    N::Error: fmt::Debug,
    N::Target: Clone,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = ReconnectService<IntoNewService<N>>;

    fn new_client(&self, target: &N::Target) -> Result<Self::Client, N::Error> {
        let new_service = self.0.clone().into_new_service(target.clone())?;
        let inner = tower_reconnect::Reconnect::new(new_service.clone());
        Ok(ReconnectService {
            new_service,
            inner,
            mute_connect_error_log: false,
        })
    }
}

// ===== impl ReconnectService =====

impl<N> Service for ReconnectService<N>
where
    N: NewService + Clone,
{
    type Request = N::Request;
    type Response = N::Response;
    type Error = N::Error;
    type Future = <tower_reconnect::Reconnect<N> as Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self.inner.poll_ready() {
            Ok(Async::NotReady) => Ok(Async::NotReady),

            Err(tower_reconnect::Error::Connect(err)) => {
                // A connection could not be established to the target.

                // This is only logged as a warning at most once. Subsequent
                // errors are logged at debug.
                if !self.mute_connect_error_log {
                    self.mute_connect_error_log = true;
                    warn!("connect error to {:?}: {}", self.endpoint, err);
                } else {
                    debug!("connect error to {:?}: {}", self.endpoint, err);
                }

                // Replace the inner service, but don't poll it immediately.
                // Instead, reschedule the task to be polled again only if the
                // caller decides not to drop it.
                //
                // This may prevent busy-looping in some situations, as well.
                self.inner = tower_reconnect::Reconnect::new(self.new_service.clone());
                task::current().notify();
                Ok(Async::NotReady)
            }

            Err(tower_reconnect::Error::Inner(err)) => {
                trace!("poll_ready: inner error, debouncing");
                self.mute_connect_error_log = false;
                Err(err)
            },

            Ok(ready) => {
                trace!("poll_ready: ready for business");
                self.mute_connect_error_log = false;
                Ok(ready)
            },
        }
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        ResponseFuture(self.inner.call(request)
    }
}
