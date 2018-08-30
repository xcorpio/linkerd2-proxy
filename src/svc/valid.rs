use std::fmt;

use super::NewClient;

/// A utility that stores a validated `NewClient` and target.
#[derive(Debug)]
pub(super) struct ValidNewClient<N: NewClient> {
    new_client: N,
    target: N::Target,
}

impl<N> ValidNewClient<N>
where
    N: NewClient,
    N::Error: fmt::Debug,
{
    pub fn new(new_client: N, target: N::Target) -> Result<(N::Client, Self), N::Error> {
        let client = new_client.new_client(&target)?;
        let valid = ValidNewClient { new_client, target };
        Ok((client, valid))
    }

    pub fn target(&self) -> &N::Target {
        &self.target
    }
}

// ==== ValidNewClient ====

impl<N> ValidNewClient<N>
where
    N: NewClient,
    N::Error: fmt::Debug,
{
    pub fn new_client(&mut self) -> N::Client {
        self.new_client
            .new_client(&self.target)
            .expect("target must be valid")
    }
}

