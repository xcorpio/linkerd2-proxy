use futures::Poll;
use futures::future::Either as EitherFuture;
use tower_service::Service;

use super::NewClient;

/// A client that may be one of two concrete types.
pub enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<A, B> NewClient for Either<A, B>
where
    A: NewClient,
    B: NewClient<Target = A::Target, Error = A::Error>,
{
    type Target = A::Target;
    type Error = A::Error;
    type Client = Either<A::Client, B::Client>;

    fn new_client(&mut self, target: &Self::Target) -> Result<Self::Client, Self::Error> {
        match self {
            Either::Left(ref mut a) => a.new_client(target).map(Either::Left),
            Either::Right(ref mut b) => b.new_client(target).map(Either::Right),
        }
    }
}

impl<A, B> Service for Either<A, B>
where
    A: Service,
    B: Service<
        Request = A::Request,
        Response = A::Response,
        Future = EitherFuture<A::Future, B::Future>,
    >,
{
    type Request = A::Request;
    type Response = A::Response;
    type Error = A::Error;
    type Future = EitherFuture<A::Future, B::Future>;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        match self {
            Either::Left(ref mut a) => a.poll_ready(),
            Either::Right(ref mut b) => b.poll_ready(),
        }
    }

    fn call(&mut self, req: Self::Request) -> Poll<(), Self::Error> {
        match self {
            Either::Left(ref mut a) => EitherFuture::Left(a.call(req)),
            Either::Right(ref mut b) => EitherFuture::Right(b.call(req)),
        }
    }
}
