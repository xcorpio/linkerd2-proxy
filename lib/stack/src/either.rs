use futures::future::Either as EitherFuture;
use futures::Poll;

use svc;

/// A client that may be one of two concrete types.
#[derive(Debug)]
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<T, U, A, B, N> super::Layer<T, U, N> for Either<A, B>
where
    A: super::Layer<T, U, N>,
    B: super::Layer<T, U, N, Error = A::Error>,
    N: super::Make<U>
{
    type Value = <Either<A::Make, B::Make> as super::Make<T>>::Value;
    type Error = <Either<A::Make, B::Make> as super::Make<T>>::Error;
    type Make = Either<A::Make, B::Make>;

    fn bind(&self, next: N) -> Self::Make {
        match self {
            Either::A(ref a) => Either::A(a.bind(next)),
            Either::B(ref b) => Either::B(b.bind(next)),
        }
    }
}

impl<T, N, M> super::Make<T> for Either<N, M>
where
    N: super::Make<T>,
    M: super::Make<T, Error = N::Error>,
{
    type Value = Either<N::Value, M::Value>;
    type Error = N::Error;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        match self {
            Either::A(ref a) => a.make(target).map(Either::A),
            Either::B(ref b) => b.make(target).map(Either::B),
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
