use std::marker::PhantomData;
use tower_service::Service;

use super::either::Either;
use super::{Stack, MakeService};

/// a `Stack` that
pub struct Mod<M: Stack<N>, N: MakeService>(Option<M>, PhantomData<N>);

impl<M, N> Mod<M, N>
where
    M: Stack<N>,
    N: MakeService<Config = M::Config, Error = M::Error>,
{
    pub fn some(m: M) -> Self {
        Mod(Some(m), PhantomData)
    }

    pub fn none() -> Self {
        Mod(None, PhantomData)
    }
}

impl<M, N> Stack<N> for Mod<M, N>
where
    M: Stack<N>,
    N: MakeService<Config = M::Config, Error = M::Error>,
    <M::MakeService as MakeService>::Service: Service<
        Request = <N::Service as Service>::Request,
        Response = <N::Service as Service>::Response,
        Error = <N::Service as Service>::Error,
    >,
{
    type Config = M::Config;
    type Error = M::Error;
    type Service = <Either<M::MakeService, N> as MakeService>::Service;
    type MakeService = Either<M::MakeService, N>;

    fn build(&self, next: N) -> Self::MakeService {
        match self.0.as_ref() {
            Some(ref m) => Either::A(m.build(next)),
            None => Either::B(next),
        }
    }
}
