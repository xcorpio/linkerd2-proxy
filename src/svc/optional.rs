use std::marker::PhantomData;
use tower_service::Service;

use super::either::Either;
use super::{MakeClient, NewClient};

/// a `MakeClient` that
pub struct Make<M: MakeClient<N>, N: NewClient>(Option<M>, PhantomData<N>);

impl<M, N> Make<M, N>
where
    M: MakeClient<N>,
    N: NewClient<Target = M::Target, Error = M::Error>,
{
    pub fn some(m: M) -> Self {
        Make(Some(m), PhantomData)
    }

    pub fn none() -> Self {
        Make(None, PhantomData)
    }
}

impl<M, N> MakeClient<N> for Make<M, N>
where
    M: MakeClient<N>,
    N: NewClient<Target = M::Target, Error = M::Error>,
    <M::NewClient as NewClient>::Client: Service<
        Request = <N::Client as Service>::Request,
        Response = <N::Client as Service>::Response,
        Error = <N::Client as Service>::Error,
    >,
{
    type Target = M::Target;
    type Error = M::Error;
    type Client = <Either<M::NewClient, N> as NewClient>::Client;
    type NewClient = Either<M::NewClient, N>;

    fn make_client(&self, next: N) -> Self::NewClient {
        match self.0.as_ref() {
            Some(ref m) => Either::A(m.make_client(next)),
            None => Either::B(next),
        }
    }
}
