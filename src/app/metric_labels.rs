use std::{fmt, net};

use metrics::FmtLabels;

use super::{inbound, outbound, Destination};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EndpointLabels {
            addr: net::SocketAddr,
            dst: Destination,
            labels: String,
}

impl From<inbound::Endpoint> for EndpointLabels {
    fn from(ep: inbound::Endpoint) -> Self {
        Self {
            addr: ep.addr,
            dst: ep.dst,
            labels: "TO=DO".to_owned(),
        }
    }
}

impl From<outbound::Endpoint> for EndpointLabels {
    fn from(ep: outbound::Endpoint) -> Self {
        let labels = "FIX=ME".to_owned();
        Self {
            addr: ep.connect.addr,
            dst: ep.dst,
            labels,
        }
    }
}

impl FmtLabels for EndpointLabels {
    fn fmt_labels(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.pad(&self.labels)
    }
}
