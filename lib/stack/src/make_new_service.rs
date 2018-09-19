use futures::future;
use svc;

use super::Make;

#[derive(Clone, Debug)]
pub struct MakeNewService<T, M: Make<T>> {
    make: M,
    target: T,
}

impl<T, M> MakeNewService<T, M>
where
    M: Make<T>,
    M::Output: svc::Service,
{
    pub fn new(make: M, target: T) -> MakeNewService<T, M> {
        Self { make, target }
    }
}

impl<T, M> svc::NewService for MakeNewService<T, M>
where
    M: Make<T>,
    M::Output: svc::Service,
{
    type Request = <M::Output as svc::Service>::Request;
    type Response = <M::Output as svc::Service>::Response;
    type Error = <M::Output as svc::Service>::Error;
    type Service = M::Output;
    type InitError = M::Error;
    type Future = future::FutureResult<Self::Service, Self::InitError>;

    fn new_service(&self) -> Self::Future {
        future::result(self.make.make(&self.target))
    }
}
