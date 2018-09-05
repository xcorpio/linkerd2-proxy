use std::fmt;

use super::MakeService;

/// A utility that stores a validated `MakeService` and target.
#[derive(Clone, Debug)]
pub(super) struct ValidMakeService<N: MakeService> {
    make_service: N,
    target: N::Config,
}

impl<N> ValidMakeService<N>
where
    N: MakeService + Clone,
    N::Config: Clone,
    N::Error: fmt::Debug,
{
    pub fn try(make_service: &N, target: &N::Config) -> Result<(N::Service, Self), N::Error> {
        let client = make_service.make_service(target)?;
        let valid = ValidMakeService {
            make_service: make_service.clone(),
            target: target.clone(),
        };
        Ok((client, valid))
    }

    pub fn target(&self) -> &N::Config {
        &self.target
    }
}

// ==== ValidMakeService ====

impl<N> ValidMakeService<N>
where
    N: MakeService,
    N::Error: fmt::Debug,
{
    pub fn make_service(&self) -> N::Service {
        self.make_service
            .make_service(&self.target)
            .expect("target must be valid")
    }
}
