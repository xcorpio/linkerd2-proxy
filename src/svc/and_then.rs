use std::marker::PhantomData;

use super::{MakeClient, NewClient};

/// Combines two `MakeClients` into one layer.
pub struct AndThen<Outer, Inner, Next>
where
    Outer: MakeClient<Inner::NewClient>,
    Inner: MakeClient<Next>,
    Next: NewClient,
{
    outer: Outer,
    inner: Inner,
    _p: PhantomData<Next>,
}

impl<Outer, Inner, Next> AndThen<Outer, Inner, Next>
where
    Outer: MakeClient<Inner::NewClient>,
    Inner: MakeClient<Next>,
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

impl<Outer, Inner, Next> MakeClient<Next> for AndThen<Outer, Inner, Next>
where
    Outer: MakeClient<Inner::NewClient>,
    Inner: MakeClient<Next>,
    Next: NewClient,
{
    type Target = Outer::Target;
    type Error = Outer::Error;
    type Client = Outer::Client;
    type NewClient = Outer::NewClient;

    fn make_client(&self, next: Next) -> Self::NewClient {
        self.outer.make_client(self.inner.make_client(next))
    }
}
