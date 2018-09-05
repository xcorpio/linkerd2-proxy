use futures::future::Either as EitherFuture;
use futures::Poll;
use tower_service::Service;

use super::MakeService;

/// A client that may be one of two concrete types.
#[derive(Debug)]
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<A, B> MakeService for Either<A, B>
where
    A: MakeService,
    B: MakeService<Config = A::Config, Error = A::Error>,
    B::Service: Service<
        Request = <A::Service as Service>::Request,
        Response = <A::Service as Service>::Response,
        Error = <A::Service as Service>::Error,
    >,
{
    type Config = A::Config;
    type Error = A::Error;
    type Service = Either<A::Service, B::Service>;

    fn make_service(&self, config: &Self::Config) -> Result<Self::Service, Self::Error> {
        match self {
            Either::A(ref a) => a.make_service(config).map(Either::A),
            Either::B(ref b) => b.make_service(config).map(Either::B),
        }
    }
}

impl<A, B> Service for Either<A, B>
where
    A: Service,
    B: Service<Request = A::Request, Response = A::Response, Error = A::Error>,
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
