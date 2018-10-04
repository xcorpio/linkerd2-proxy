use futures::Poll;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub enum Update<Endpoint> {
    Stack(SocketAddr, Endpoint),
    Remove(SocketAddr),
}

pub trait Resolve<Target> {
    type Endpoint;
    type Resolution: Resolution<Endpoint = Self::Endpoint>;

    fn resolve(&self, target: &Target) -> Self::Resolution;
}

pub trait Resolution {
    type Endpoint;
    type Error;

    fn poll(&mut self) -> Poll<Update<Self::Endpoint>, Self::Error>;
}
