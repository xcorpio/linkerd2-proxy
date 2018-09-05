use std::marker::PhantomData;

use super::{Stack, MakeService};

/// Combines two `Stacks` into one layer.
pub struct AndThen<Outer, Inner, Next>
where
    Outer: Stack<Inner::MakeService>,
    Inner: Stack<Next>,
    Next: MakeService,
{
    outer: Outer,
    inner: Inner,
    _p: PhantomData<Next>,
}

impl<Outer, Inner, Next> AndThen<Outer, Inner, Next>
where
    Outer: Stack<Inner::MakeService>,
    Inner: Stack<Next>,
    Next: MakeService,
{
    pub(super) fn new(outer: Outer, inner: Inner) -> Self {
        Self {
            outer,
            inner,
            _p: PhantomData,
        }
    }
}

impl<Outer, Inner, Next> Stack<Next> for AndThen<Outer, Inner, Next>
where
    Outer: Stack<Inner::MakeService>,
    Inner: Stack<Next>,
    Next: MakeService,
{
    type Config = Outer::Config;
    type Error = Outer::Error;
    type Service = Outer::Service;
    type MakeService = Outer::MakeService;

    fn build(&self, next: Next) -> Self::MakeService {
        self.outer.build(self.inner.build(next))
    }
}
