use std::marker::PhantomData;

use super::{MakeClient, NewClient};

/// Combines two `MakeClients` into one layer.
pub struct AndThen<A, B, N>
where
    A: MakeClient<B::NewClient>,
    B: MakeClient<N>,
    N: NewClient,
{
    outer: A,
    inner: B,
    _p: PhantomData<N>,
}

impl<A, B, N> MakeClient<N> for AndThen<A, B, N>
where
    A: MakeClient<B::NewClient>,
    B: MakeClient<N>,
    N: NewClient,
{
    type Target = A::Target;
    type Error = A::Error;
    type Client = A::Client;
    type NewClient = A::NewClient;

    fn make_client(&self, next: N) -> Self::NewClient {
        self.outer.make_client(self.inner.make_client(next))
    }
}
