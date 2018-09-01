use futures::{future, Future, Poll};
use http;
use http::header::{TRANSFER_ENCODING, HeaderValue};

use svc::{Service, http::h1};
use super::L5D_ORIG_PROTO;

/// Downgrades HTTP2 requests that were previously upgraded to their original
/// protocol.
#[derive(Debug)]
pub struct Downgrade<S> {
    inner: S,
}

// ===== impl Upgrade =====

impl<S> Downgrade<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
        }
    }
}

impl<S, B1, B2> Service for Downgrade<S>
where
    S: Service<Request = http::Request<B1>, Response = http::Response<B2>>,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = future::Map<
        S::Future,
        fn(S::Response) -> S::Response
    >;

    fn poll_ready(&mut self) -> Poll<(), Self::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, mut req: Self::Request) -> Self::Future {
        let mut upgrade_response = false;

        if req.version() == http::Version::HTTP_2 {
            if let Some(orig_proto) = req.headers_mut().remove(L5D_ORIG_PROTO) {
                debug!("translating HTTP2 to orig-proto: {:?}", orig_proto);

                let val: &[u8] = orig_proto.as_bytes();

                if val.starts_with(b"HTTP/1.1") {
                    *req.version_mut() = http::Version::HTTP_11;
                } else if val.starts_with(b"HTTP/1.0") {
                    *req.version_mut() = http::Version::HTTP_10;
                } else {
                    warn!(
                        "unknown {} header value: {:?}",
                        L5D_ORIG_PROTO,
                        orig_proto,
                    );
                }

                if !was_absolute_form(val) {
                    h1::set_origin_form(req.uri_mut());
                }
                upgrade_response = true;
            }
        }

        let fut = self.inner.call(req);

        if upgrade_response {
            fut.map(|mut res| {
                let orig_proto = if res.version() == http::Version::HTTP_11 {
                    "HTTP/1.1"
                } else if res.version() == http::Version::HTTP_10 {
                    "HTTP/1.0"
                } else {
                    return res;
                };

                res.headers_mut().insert(
                    L5D_ORIG_PROTO,
                    HeaderValue::from_static(orig_proto)
                );

                // transfer-encoding is illegal in HTTP2
                res.headers_mut().remove(TRANSFER_ENCODING);

                *res.version_mut() = http::Version::HTTP_2;
                res
            })
        } else {
            fut.map(|res| res)
        }
    }
}

fn was_absolute_form(val: &[u8]) -> bool {
    val.len() >= "HTTP/1.1; absolute-form".len()
        && &val[10..23] == b"absolute-form"
}
