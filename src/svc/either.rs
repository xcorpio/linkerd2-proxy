use futures::Poll;
use futures::future::Either as EitherFuture;
use tower_service::Service;

use super::NewClient;

/// A client that may be one of two concrete types.
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<A, B> NewClient for Either<A, B>
where
    A: NewClient,
    B: NewClient<Target = A::Target, Error = A::Error>,
    B::Client: Service<
        Request = <A::Client as Service>::Request,
        Response = <A::Client as Service>::Response,
        Error = <A::Client as Service>::Error,
    >,
{
    type Target = A::Target;
    type Error = A::Error;
    type Client = Either<A::Client, B::Client>;

    fn new_client(&self, target: &Self::Target) -> Result<Self::Client, Self::Error> {
        match self {
            Either::A(ref a) => a.new_client(target).map(Either::A),
            Either::B(ref b) => b.new_client(target).map(Either::B),
        }
    }
}

impl<A, B> Service for Either<A, B>
where
    A: Service,
    B: Service<
        Request = A::Request,
        Response = A::Response,
        Error = A::Error,
    >,
{
    type Request = A::Request;
    type Response = A::Response;
    type Error = A::Error;
    type Future = EitherFuture<A::Future, B::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self {
            Either::A(ref mut a) => a.poll_ready(),
            Either::B(ref mut b) => b.poll_ready(),
        }
    }

    fn call(&mut self, req: Self::Request) -> Self::Future {
        match self {
            Either::A(ref mut a) => EitherFuture::A(a.call(req)),
            Either::B(ref mut b) => EitherFuture::B(b.call(req)),
        }
    }
}
