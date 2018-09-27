use http;
use std::{fmt::{self, Write}, net};

use metrics::FmtLabels;
use Conditional;
use transport::tls;

use super::{classify, inbound, outbound};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EndpointLabels {
    addr: net::SocketAddr,
    direction: Direction,
    tls_status: tls::Status,
    authority: Authority,
    labels: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum Direction { In, Out }

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Authority(Option<http::uri::Authority>);

impl From<inbound::Endpoint> for EndpointLabels {
    fn from(ep: inbound::Endpoint) -> Self {
        Self {
            addr: ep.addr,
            authority: Authority(ep.authority),
            direction: Direction::In,
            tls_status: ep.source_tls_status,
            labels: "".to_owned(),
        }
    }
}

impl From<outbound::Endpoint> for EndpointLabels {
    fn from(ep: outbound::Endpoint) -> Self {
        let mut label_iter = ep.metadata.labels().into_iter();
        let labels = if let Some((k0, v0)) = label_iter.next() {
            let mut s = format!("dst_{}=\"{}\"", k0, v0);
            for (k, v) in label_iter {
                write!(s, ",dst_{}=\"{}\"", k, v)
                    .expect("label concat must succeed");
            }
            s
        } else {
            "".to_owned()
        };

        Self {
            addr: ep.connect.addr,
            authority: Authority(ep.dst.authority),
            direction: Direction::Out,
            tls_status: ep.connect.tls_status(),
            labels,
        }
    }
}

impl FmtLabels for EndpointLabels {
    fn fmt_labels(&self, f: &mut fmt::Formatter) -> fmt::Result {
        ((&self.authority, &self.direction), &self.tls_status)
            .fmt_labels(f)?;

        if !self.labels.is_empty() {
            write!(f, ",{}", self.labels)?;
        }

        Ok(())
    }
}

impl FmtLabels for Direction {
    fn fmt_labels(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Direction::In => write!(f, "direction=\"inbound\""),
            Direction::Out => write!(f, "direction=\"outbound\""),
        }
    }
}

impl FmtLabels for Authority {
    fn fmt_labels(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            Some(ref a) => write!(f, "authority=\"{}\"", a),
            None => f.pad("authority=\"\""),
        }
    }
}

impl FmtLabels for classify::Class {
    fn fmt_labels(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "classification=\"success\",status_code=\"200\"")
    }
}

impl FmtLabels for tls::Status {
    fn fmt_labels(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Conditional::None(tls::ReasonForNoTls::NoIdentity(why)) =>
                write!(f, "tls=\"no_identity\",no_tls_reason=\"{}\"", why),
            status =>
                write!(f, "tls=\"{}\"", status),
        }
    }
}
