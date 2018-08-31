use std::fmt;

use futures::{Async, Future, Poll, task};
use tower_reconnect;

use super::{MakeClient, NewClient, Service, IntoNewService};

#[derive(Copy, Clone, Debug)]
pub struct Make;

#[derive(Clone, Debug)]
pub struct Reconnect<N: NewClient>(N);

pub struct ReconnectService<N>
where
    N: NewClient,
    N::Target: fmt::Debug,
    N::Error: fmt::Display,
{
    inner: tower_reconnect::Reconnect<IntoNewService<N>>,

    /// The connection target, used for debug logging.
    target: N::Target,

    /// Prevents logging repeated connect errors.
    ///
    /// Set back to false after a connect succeeds, to log about future errors.
    mute_connect_error_log: bool,
}

pub struct ResponseFuture<N: NewClient> {
    inner: <tower_reconnect::Reconnect<IntoNewService<N>> as Service>::Future,
}

// ===== impl Make =====

impl<N> MakeClient<N> for Make
where
    N: NewClient + Clone,
    N::Target: Clone + fmt::Debug,
    N::Error: fmt::Display,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = ReconnectService<N>;
    type NewClient = Reconnect<N>;

    fn make_client(&self, next: N) -> Self::NewClient {
        Reconnect(next)
    }
}

// ===== impl Reconnect =====

impl<N> NewClient for Reconnect<N>
where
    N: NewClient + Clone,
    N::Target: Clone + fmt::Debug,
    N::Error: fmt::Display,
{
    type Target = N::Target;
    type Error = N::Error;
    type Client = ReconnectService<N>;

    fn new_client(&self, target: &N::Target) -> Result<Self::Client, N::Error> {
        let new_service = self.0.clone().into_new_service(target.clone());
        let inner = tower_reconnect::Reconnect::new(new_service);
        Ok(ReconnectService {
            target: target.clone(),
            inner,
            mute_connect_error_log: false,
        })
    }
}

// ===== impl ReconnectService =====

impl<N> Service for ReconnectService<N>
where
    N: NewClient,
    N::Target: fmt::Debug,
    N::Error: fmt::Display,
{
    type Request = <N::Client as Service>::Request;
    type Response = <N::Client as Service>::Response;
    type Error = <N::Client as Service>::Error;
    type Future = ResponseFuture<N>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self.inner.poll_ready() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(ready) => {
                trace!("poll_ready: ready for business");
                self.mute_connect_error_log = false;
                Ok(ready)
            },

            Err(tower_reconnect::Error::Inner(err)) => {
                trace!("poll_ready: inner error, debouncing");
                self.mute_connect_error_log = false;
                Err(err)
            },

            Err(tower_reconnect::Error::Connect(err)) => {
                // A connection could not be established to the target.

                // This is only logged as a warning at most once. Subsequent
                // errors are logged at debug.
                if !self.mute_connect_error_log {
                    self.mute_connect_error_log = true;
                    warn!("connect error to {:?}: {}", self.target, err);
                } else {
                    debug!("connect error to {:?}: {}", self.target, err);
                }

                // The inner service is now idle and will renew its internal
                // state on the next poll. Instead of doing this immediately,
                // the task is scheduled to be polled again only if the caller
                // decides not to drop it.
                //
                // This prevents busy-looping when the connect error is
                // instantaneous.
                task::current().notify();
                Ok(Async::NotReady)
            }

            Err(tower_reconnect::Error::NotReady) => {
                unreachable!("poll_ready can't fail with NotReady");
            }
        }
    }

    fn call(&mut self, request: Self::Request) -> Self::Future {
        ResponseFuture {
            inner: self.inner.call(request),
        }
    }
}

impl<N: NewClient> Future for ResponseFuture<N> {
    type Item = <N::Client as Service>::Response;
    type Error = <N::Client as Service>::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll().map_err(|e| match e {
            tower_reconnect::Error::Inner(err) => err,
            _ => unreachable!("response future must fail with inner error"),
        })
    }
}
