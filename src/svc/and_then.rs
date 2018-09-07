use std::marker::PhantomData;

use super::{Stack, NewClient};

/// Combines two `Stacks` into one layer.
pub struct AndThen<Outer, Inner, Next>
where
    Outer: Stack<Inner::NewClient>,
    Inner: Stack<Next>,
    Next: NewClient,
{
    outer: Outer,
    inner: Inner,
    _p: PhantomData<Next>,
}

impl<Outer, Inner, Next> AndThen<Outer, Inner, Next>
where
    Outer: Stack<Inner::NewClient>,
    Inner: Stack<Next>,
    Next: NewClient,
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
    Outer: Stack<Inner::NewClient>,
    Inner: Stack<Next>,
    Next: NewClient,
{
    type Config = Outer::Config;
    type Error = Outer::Error;
    type Service = Outer::Service;
    type NewClient = Outer::NewClient;

    fn build(&self, next: Next) -> Self::NewClient {
        self.outer.build(self.inner.build(next))
    }
}
