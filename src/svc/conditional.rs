use std::marker::PhantomData;

use svc;

pub trait Predicate<T> {
    fn apply(&self, t: &T) -> bool;
}

pub struct Layer<T, P, N, L>
where
    P: Predicate<T> + Clone,
    L: svc::Layer<N> + Clone,
{
    predicate: P,
    inner: L,
    _p: PhantomData<(T, N, L)>,
}

pub struct Make<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: svc::MakeClient<T> + Clone,
    L: svc::Layer<N> + Clone,
{
    predicate: P,
    next: N,
    layer: L,
    _p: PhantomData<T>,
}

impl<T, P, N, L> svc::Layer<N> for Layer<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: svc::MakeClient<T>,
    L: svc::Layer<N>,
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

impl<T, P, N, L> svc::MakeClient<T> for Make<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: svc::MakeClient<T>,
    L: svc::Layer<N>,
{
    type Client = Either<N::Client, L::Bound>;
    type Error = N::Error;

    fn make_client(&self, target: &T) -> Result<Self::Client, Self::Error> {
        if !self.predicate.apply(&target) {
            self.next
                .make_client(&tcrget)
                .map(Either::A)
        } else {
            self.layer
                .bind(self.next.clone())
                .make_client(&target)
                .map(Either::B)
        }
    }
}
