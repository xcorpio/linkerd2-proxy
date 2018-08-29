use indexmap::IndexMap;
use std::net::SocketAddr;

use tls;
use conditional::Conditional;

/// An individual traffic target.
///
/// Equality, Ordering, and hashability is determined solely by the Endpoint's address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    address: SocketAddr,
    metadata: Metadata,
}

/// Metadata describing an endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    /// Arbitrary endpoint labels. Primarily used for telemetry.
    labels: IndexMap<String, String>,

    /// A hint from the controller about what protocol (HTTP1, HTTP2, etc) the
    /// destination understands.
    protocol_hint: ProtocolHint,

    /// How to verify TLS for the endpoint.
    tls_identity: Conditional<tls::Identity, tls::ReasonForNoIdentity>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolHint {
    /// We don't what the destination understands, so forward messages in the
    /// protocol we received them in.
    Unknown,
    /// The destination can receive HTTP2 messages.
    Http2,
}

// ==== impl Endpoint =====

impl Endpoint {
    pub fn new(address: SocketAddr, metadata: Metadata) -> Self {
        Self {
            address,
            metadata,
        }
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn labels(&self) -> &IndexMap<String, String> {
        self.metadata.labels()
    }

    pub fn can_use_orig_proto(&self) -> bool {
        match self.metadata.protocol_hint() {
            ProtocolHint::Unknown => false,
            ProtocolHint::Http2 => true,
        }
    }

    pub fn tls_identity(&self) -> Conditional<&tls::Identity, tls::ReasonForNoIdentity> {
        self.metadata.tls_identity()
    }
}

impl From<SocketAddr> for Endpoint {
    fn from(address: SocketAddr) -> Self {
        Self {
            address,
            metadata: Metadata::no_metadata()
        }
    }
}

// ===== impl Metadata =====

impl Metadata {
    /// Construct a Metadata struct representing an endpoint with no metadata.
    pub fn no_metadata() -> Self {
        Self {
            labels: IndexMap::default(),
            protocol_hint: ProtocolHint::Unknown,
            // If we have no metadata on an endpoint, assume it does not support TLS.
            tls_identity:
                Conditional::None(tls::ReasonForNoIdentity::NotProvidedByServiceDiscovery),
        }
    }

    pub fn new(
        labels: IndexMap<String, String>,
        protocol_hint: ProtocolHint,
        tls_identity: Conditional<tls::Identity, tls::ReasonForNoIdentity>
    ) -> Self {
        Self {
            labels,
            protocol_hint,
            tls_identity,
        }
    }

    /// Returns the endpoint's labels from the destination service, if it has them.
    pub fn labels(&self) -> &IndexMap<String, String> {
        &self.labels
    }

    pub fn protocol_hint(&self) -> ProtocolHint {
        self.protocol_hint
    }

    pub fn tls_identity(&self) -> Conditional<&tls::Identity, tls::ReasonForNoIdentity> {
        self.tls_identity.as_ref()
    }
}
