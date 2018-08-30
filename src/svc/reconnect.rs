use std::fmt;
use std::marker::PhantomData;

use futures::{Async, Future, Poll, future, task};
use tower_reconnect;
use tower_service::Service;

use super::{NewClient, Service, ValidNewClient};

pub struct Reconnect<N: NewClient>(N);

pub struct ReconnectService<N: NewClient> {
    new_client: ValidNewClient<N>,

    current: tower_reconnect::Reconnect<N::Client>,

    /// Prevents logging repeated connect errors.
    ///
    /// Set back to false after a connect succeeds, to log about future errors.
    mute_connect_error_log: bool,
}

// ===== impl Reconnect =====

impl<N> NewClient for Reconnect<N>
where
    N: NewClient + Clone,
    N::Target: Clone,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = ReconnectService<N>;

    fn new_client(&mut self, target: &N::Target) -> Result<Self::Client, N::Error> {
        let (current, new_client) = ValidNewClient::new(self.clone(), target.clone())?;
        Ok(ReconnectService {
            new_client,
            current: tower_reconnect::Reconnect::new(current),
            mute_connect_error_log: false,
        })
    }
}

// ===== impl ReconnectService =====

impl<N> Service for ReconnectService<N>
where
    N: NewClient,
{
    type Request = <N::Client as Service>::Request;
    type Response = <N::Client as Service>::Response;
    type Error = <N::Client as Service>::Error;
    type Future = <N::Client as Service>::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self.current.poll_ready() {
            Ok(Async::NotReady) => Ok(Async::NotReady),

            Err(tower_reconnect::Error::Connect(err)) => {
                if !self.mute_connect_error_log {
                    self.mute_connect_error_log = true;
                    warn!("connect error to {:?}: {}", self.endpoint, err);
                } else {
                    debug!("connect error to {:?}: {}", self.endpoint, err);
                }

                let new = self.new_client.new_client();
                self.current = tower_reconnect::Reconnect::new(new);

                task::current().notify();
                Ok(Async::NotReady)
            }

            Err(tower_reconnect::Error::Inner(err)) => {
                trace!("poll_ready: inner error, debouncing");
                self.mute_connect_error_log = false;
                Err(err)
            },

            ready @ Ok(Async::Reaady(_)) => {
                trace!("poll_ready: ready for business");
                self.mute_connect_error_log = false;
                ready
            },
        }
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        self.current.call(request)
    }
}
