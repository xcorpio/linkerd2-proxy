use std::marker::PhantomData;
use svc;

pub trait Predicate<T> {
    fn apply(&self, t: &T) -> bool;
}

pub struct Layer<T, P, N, L>
where
    P: Predicate<T> + Clone,
    L: super::Layer<N> + Clone,
{
    predicate: P,
    inner: L,
    _p: PhantomData<(T, N, L)>,
}

pub struct Make<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: super::Make<T> + Clone,
    L: super::Layer<N>,
{
    predicate: P,
    next: N,
    layer: L,
    _p: PhantomData<T>,
}

impl<T, P, N, L> super::Layer<N> for Layer<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: super::Make<T> + Clone,
    N::Output: svc::Service,
    L: super::Layer<N> + Clone,
    L::Bound: super::Make<T>,
    <L::Bound as super::Make<T>>::Output: svc::Service<
        Request = <N::Output as svc::Service>::Request,
        Response = <N::Output as svc::Service>::Response,
        Error = <N::Output as svc::Service>::Error,
    >,
{
    type Bound = Make<T, P, N, L>;

    fn bind(&self, next: N) -> Self::Bound {
        Make {
            predicate: self.predicate.clone(),
            next,
            layer: self.inner.clone(),
            _p: PhantomData,
        }
    }
}

impl<T, P, N, L> super::Make<T> for Make<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: super::Make<T> + Clone,
    N::Output: svc::Service,
    L: super::Layer<N>,
    L::Bound: super::Make<T, Error = N::Error>,
    <L::Bound as super::Make<T>>::Output: svc::Service<
        Request = <N::Output as svc::Service>::Request,
        Response = <N::Output as svc::Service>::Response,
        Error = <N::Output as svc::Service>::Error,
    >,
{
    type Output = super::Either<N::Output, <L::Bound as super::Make<T>>::Output>;
    type Error = N::Error;

    fn make(&self, target: &T) -> Result<Self::Output, Self::Error> {
        if !self.predicate.apply(&target) {
            self.next
                .make(&target)
                .map(super::Either::A)
        } else {
            self.layer
                .bind(self.next.clone())
                .make(&target)
                .map(super::Either::B)
        }
    }
}
