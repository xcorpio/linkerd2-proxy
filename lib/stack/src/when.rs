use std::marker::PhantomData;

pub trait Predicate<T> {
    fn apply(&self, t: &T) -> bool;
}

pub struct Layer<T, P, N, L>
where
    P: Predicate<T> + Clone,
    L: super::Layer<T, T, N> + Clone,
    N: super::Make<T>,
{
    predicate: P,
    inner: L,
    _p: PhantomData<(T, N, L)>,
}

pub struct Make<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: super::Make<T> + Clone,
    L: super::Layer<T, T, N>,
{
    predicate: P,
    next: N,
    layer: L,
    _p: PhantomData<T>,
}

impl<T, P, N, L> super::Layer<T, T, N> for Layer<T, P, N, L>
where
    P: Predicate<T> + Clone,
    N: super::Make<T> + Clone,
    L: super::Layer<T, T, N> + Clone,
    L::Make: super::Make<T>,
{
    type Value = <Make<T, P, N, L> as super::Make<T>>::Value;
    type Error = <Make<T, P, N, L> as super::Make<T>>::Error;
    type Make = Make<T, P, N, L>;

    fn bind(&self, next: N) -> Self::Make {
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
    L: super::Layer<T, T, N>,
    L::Make: super::Make<T>,
{
    type Value = super::Either<N::Value, L::Value>;
    type Error = super::Either<N::Error, L::Error>;

    fn make(&self, target: &T) -> Result<Self::Value, Self::Error> {
        if !self.predicate.apply(&target) {
            self.next
                .make(&target)
                .map(super::Either::A)
                .map_err(super::Either::A)
        } else {
            self.layer
                .bind(self.next.clone())
                .make(&target)
                .map(super::Either::B)
                .map_err(super::Either::B)
        }
    }
}
