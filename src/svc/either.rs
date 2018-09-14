use futures::future::Either as EitherFuture;
use futures::Poll;

use svc;

/// A client that may be one of two concrete types.
#[derive(Debug)]
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<A, B, N> svc::Layer<N> for Either<A, B>
where
    A: svc::Layer<N>,
    B: svc::Layer<N>,
{
    type Bound = Either<A::Bound, B::Bound>;

    fn bind(&self, next: N) -> Self::Bound {
        match self {
            Either::A(ref a) => Either::A(a.bind(next)),
            Either::B(ref b) => Either::B(b.bind(next)),
        }
    }
}

impl<T, N, M> svc::MakeClient<T> for Either<N, M>
where
    N: svc::MakeClient<T>,
    M: svc::MakeClient<T, Error = N::Error>,
    M::Client: svc::Service<
        Request = <N::Client as svc::Service>::Request,
        Response = <N::Client as svc::Service>::Response,
        Error = <N::Client as svc::Service>::Error,
    >,
{
    type Client = Either<N::Client, M::Client>;
    type Error = Either<N::Error, M::Error>;

    fn make_client(&self, target: &T) -> Result<Self::Client, Self::Error> {
        match self {
            Either::A(ref a) => a.make_client(target).map(Either::A).map_err(Either::A),
            Either::B(ref b) => b.make_client(target).map(Either::B).map_err(Either::B),
        }
    }
}

impl<A, B> svc::Service for Either<A, B>
where
    A: svc::Service,
    B: svc::Service<Request = A::Request, Response = A::Response, Error = A::Error>,
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
