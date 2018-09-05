use std::fmt;

use futures::{task, Async, Future, Poll};
use tower_reconnect;

use super::{IntoNewService, Stack, MakeService, Service};

#[derive(Copy, Clone, Debug)]
pub struct Mod;

#[derive(Clone, Debug)]
pub struct Reconnect<N: MakeService>(N);

pub struct ReconnectService<N>
where
    N: MakeService,
    N::Config: fmt::Debug,
    N::Error: fmt::Display,
{
    inner: tower_reconnect::Reconnect<IntoNewService<N>>,

    /// The connection config, used for debug logging.
    config: N::Config,

    /// Prevents logging repeated connect errors.
    ///
    /// Set back to false after a connect succeeds, to log about future errors.
    mute_connect_error_log: bool,
}

pub struct ResponseFuture<N: MakeService> {
    inner: <tower_reconnect::Reconnect<IntoNewService<N>> as Service>::Future,
}

// ===== impl Make =====

impl<N> Stack<N> for Mod
where
    N: MakeService + Clone,
    N::Config: Clone + fmt::Debug,
    N::Error: fmt::Display,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = ReconnectService<N>;
    type MakeService = Reconnect<N>;

    fn build(&self, next: N) -> Self::MakeService {
        Reconnect(next)
    }
}

// ===== impl Reconnect =====

impl<N> MakeService for Reconnect<N>
where
    N: MakeService + Clone,
    N::Config: Clone + fmt::Debug,
    N::Error: fmt::Display,
{
    type Config = N::Config;
    type Error = N::Error;
    type Service = ReconnectService<N>;

    fn make_service(&self, config: &N::Config) -> Result<Self::Service, N::Error> {
        let new_service = self.0.clone().into_new_service(config.clone());
        let inner = tower_reconnect::Reconnect::new(new_service);
        Ok(ReconnectService {
            config: config.clone(),
            inner,
            mute_connect_error_log: false,
        })
    }
}

// ===== impl ReconnectService =====

impl<N> Service for ReconnectService<N>
where
    N: MakeService,
    N::Config: fmt::Debug,
    N::Error: fmt::Display,
{
    type Request = <N::Service as Service>::Request;
    type Response = <N::Service as Service>::Response;
    type Error = <N::Service as Service>::Error;
    type Future = ResponseFuture<N>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self.inner.poll_ready() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(ready) => {
                trace!("poll_ready: ready for business");
                self.mute_connect_error_log = false;
                Ok(ready)
            }

            Err(tower_reconnect::Error::Inner(err)) => {
                trace!("poll_ready: inner error, debouncing");
                self.mute_connect_error_log = false;
                Err(err)
            }

            Err(tower_reconnect::Error::Connect(err)) => {
                // A connection could not be established to the config.

                // This is only logged as a warning at most once. Subsequent
                // errors are logged at debug.
                if !self.mute_connect_error_log {
                    self.mute_connect_error_log = true;
                    warn!("connect error to {:?}: {}", self.config, err);
                } else {
                    debug!("connect error to {:?}: {}", self.config, err);
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

impl<N: MakeService> Future for ResponseFuture<N> {
    type Item = <N::Service as Service>::Response;
    type Error = <N::Service as Service>::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.inner.poll().map_err(|e| match e {
            tower_reconnect::Error::Inner(err) => err,
            _ => unreachable!("response future must fail with inner error"),
        })
    }
}
