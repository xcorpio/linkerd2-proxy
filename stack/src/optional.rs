use std::marker::PhantomData;

pub struct Optional<N, L: super::Layer<N>>(Option<L>, PhantomData<N>);

impl<N, L: super::Layer<N>> Optional<N, L> {
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

impl<N, L: super::Layer<N>> super::Layer<N> for Optional<N, L> {
    type Bound = super::Either<N, L::Bound>;

    fn bind(&self, next: N) -> Self::Bound {
        match self.0.as_ref() {
            None => super::Either::A(next),
            Some(ref m) => super::Either::B(m.bind(next)),
        }
    }
}

impl<N, L: super::Layer<N>> From<Option<L>> for Optional<N, L> {
    fn from(orig: Option<L>) -> Self {
        Optional(orig, PhantomData)
    }
}
