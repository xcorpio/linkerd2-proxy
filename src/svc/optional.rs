use super::{NewClient, MakeClient};
use super::either::Either;

pub struct Make<M: MakeClient>(Option<M>);

impl<M, N> Make<M>
where
    M: MakeClient<N>,
    N: NewClient<Target = M::Target, Error = M::Error>,
{
    pub fn with(m: M) -> Self {
        Make(Some(m))
    }

    pub fn without() -> Self {
        Make(None)
    }
}

impl<M, N> MakeClient<N> for Make<M>
where
    M: MakeClient<N>,
    N: NewClient<Target = M::Target, Error = M::Error>,
{
    type Target = M::Target;
    type Error = M::Error;
    type Client = M::Client;
    type NewClient = Either<M::NewClient, N>;

    fn make_client(&self, next: N) -> Self::NewClient {
        match self.0.as_ref() {
            Some(ref m) => Either::Left(m.make_client(next)),
            None => Either::Right(next),
        }
    }
}
