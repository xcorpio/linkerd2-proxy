use std::marker::PhantomData;

use svc;

pub struct Optional<N, L: svc::Layer<N>>(Option<L>, PhantomData<N>);

impl<N, L: svc::Layer<N>> Optional<N, L> {
    pub fn some(layer: L) -> Self {
        Optional(Some(layer), PhantomData)
    }

    pub fn none() -> Self {
        Optional(None, PhantomData)
    }

    pub fn when<F: FnOnce() -> L>(predicate: bool, mk: F) -> Self {
        if predicate {
            Self::some(mk())
        } else {
            Self::none()
        }
    }
}

impl<N, L: svc::Layer<N>> svc::Layer<N> for Optional<N, L> {
    type Bound = svc::Either<N, L::Bound>;

    fn bind(&self, next: N) -> Self::Bound {
        match self.0.as_ref() {
            None => svc::Either::A(next),
            Some(ref m) => svc::Either::B(m.bind(next)),
        }
    }
}

impl<N, L: svc::Layer<N>> From<Option<L>> for Optional<N, L> {
    fn from(orig: Option<L>) -> Self {
        Optional(orig, PhantomData)
    }
}
