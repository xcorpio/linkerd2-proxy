extern crate tower_discover;

use futures::{Async, Poll};
use std::net::SocketAddr;
use std::{error, fmt};

pub use self::tower_discover::Change;
use svc;

pub type Update<E> = Change<SocketAddr, E>;

/// Resolves `T`-typed names/addresses as a `Resolution`.
pub trait Resolve<T> {
    type Endpoint;
    type Resolution: Resolution<Endpoint = Self::Endpoint>;

    fn resolve(&self, target: &T) -> Self::Resolution;
}

/// An infinite stream of endpoint updates.
pub trait Resolution {
    type Endpoint;
    type Error;

    fn poll(&mut self) -> Poll<Update<Self::Endpoint>, Self::Error>;
}

pub trait HasEndpoint {
    type Endpoint;

    fn endpoint(&self) -> &Self::Endpoint;
}

#[derive(Clone, Debug)]
pub struct Layer<R> {
    resolve: R,
}

#[derive(Clone, Debug)]
pub struct Stack<R, M> {
    resolve: R,
    inner: M,
}

#[derive(Debug)]
pub enum Error<R, M> {
    Resolve(R),
    Stack(M),
}

/// Observes an `R`-typed resolution stream, using an `M`-typed endpoint stack to
/// build a service for each endpoint.
#[derive(Clone, Debug)]
pub struct Discover<R: Resolution, M: svc::Stack<R::Endpoint>> {
    resolution: R,
    make: M,
}

#[derive(Clone, Debug)]
pub struct EndpointService<E, S> {
    endpoint: E,
    service: S,
}

// === impl Layer ===

pub fn layer<T, R>(resolve: R) -> Layer<R>
where
    R: Resolve<T> + Clone,
    R::Endpoint: fmt::Debug + Clone,
{
    Layer { resolve }
}

impl<T, R, M> svc::Layer<T, R::Endpoint, M> for Layer<R>
where
    R: Resolve<T> + Clone,
    R::Endpoint: fmt::Debug + Clone,
    M: svc::Stack<R::Endpoint> + Clone,
    M::Value: svc::Service,
{
    type Value = <Stack<R, M> as svc::Stack<T>>::Value;
    type Error = <Stack<R, M> as svc::Stack<T>>::Error;
    type Stack = Stack<R, M>;

    fn bind(&self, inner: M) -> Self::Stack {
        Stack {
            resolve: self.resolve.clone(),
            inner,
        }
    }
}

// === impl Stack ===

impl<T, R, M> svc::Stack<T> for Stack<R, M>
where
    R: Resolve<T>,
    R::Endpoint: fmt::Debug + Clone,
    M: svc::Stack<R::Endpoint> + Clone,
    M::Value: svc::Service,
{
    type Value = Discover<R::Resolution, M>;
    type Error = Error<<R::Resolution as Resolution>::Error, M::Error>;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        let resolution = self.resolve.resolve(target);
        Ok(Discover {
            resolution,
            make: self.inner.clone(),
        })
    }
}

// === impl Discover ===

impl<R, M> tower_discover::Discover for Discover<R, M>
where
    R: Resolution,
    R::Endpoint: fmt::Debug + Clone,
    M: svc::Stack<R::Endpoint>,
    M::Value: svc::Service,
{
    type Key = SocketAddr;
    type Request = <M::Value as svc::Service>::Request;
    type Response = <M::Value as svc::Service>::Response;
    type Error = <M::Value as svc::Service>::Error;
    type Service = EndpointService<R::Endpoint, M::Value>;
    type DiscoverError = Error<R::Error, M::Error>;

    fn poll(&mut self) -> Poll<Change<SocketAddr, Self::Service>, Self::DiscoverError> {
        loop {
            match try_ready!(self.resolution.poll().map_err(Error::Resolve)) {
                Change::Insert(addr, endpoint) => {
                    trace!("discover: insert: addr={}; endpoint={:?}", addr, endpoint);
                    let service = self.make.make(&endpoint).map_err(Error::Stack)?;
                    let svc = EndpointService { endpoint, service };
                    return Ok(Async::Ready(Change::Insert(addr, svc)));
                }
                Change::Remove(addr) => {
                    trace!("discover: remove: addr={}", addr);
                    return Ok(Async::Ready(Change::Remove(addr)));
                }
            }
        }
    }
}

// === impl EndpointService ===

impl<E, S> HasEndpoint for EndpointService<E, S> {
    type Endpoint = E;

    fn endpoint(&self) -> &Self::Endpoint {
        &self.endpoint
    }
}

impl<E, S> svc::Service for EndpointService<E, S>
where
    S: svc::Service,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.service.poll_ready()
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        self.service.call(req)
    }
}

// === impl Error ===

impl<M> fmt::Display for Error<(), M>
where
    M: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Resolve(()) => unreachable!("resolution must succeed"),
            Error::Stack(e) => e.fmt(f),
        }
    }
}

impl<M> error::Error for Error<(), M> where M: error::Error {}
