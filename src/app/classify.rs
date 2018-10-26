use h2;
use http;
use std::sync::Arc;

use proxy::http::{classify, profiles};

#[derive(Clone, Debug, Default)]
pub struct Classify {
    classes: Arc<Vec<profiles::ResponseClass>>,
}

#[derive(Clone, Debug, Default)]
pub struct ClassifyResponse {
    classes: Arc<Vec<profiles::ResponseClass>>,
}

#[derive(Clone, Debug, Default)]
pub struct ClassifyEos {
    status: http::StatusCode,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Class {
    Grpc(SuccessOrFailure, u32),
    Http(SuccessOrFailure, http::StatusCode),
    Stream(SuccessOrFailure, String),
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum SuccessOrFailure {
    Success,
    Failure,
}

// === impl Classify ===

impl Classify {
    pub fn new(classes: Vec<profiles::ResponseClass>) -> Self {
        Self {
            classes: Arc::new(classes),
        }
    }
}

impl classify::Classify for Classify {
    type Class = Class;
    type Error = h2::Error;
    type ClassifyResponse = ClassifyResponse;
    type ClassifyEos = ClassifyEos;

    fn classify<B>(&self, _: &http::Request<B>) -> Self::ClassifyResponse {
        ClassifyResponse {
            classes: self.classes.clone(),
        }
    }
}

// === impl ClassifyResponse ===

impl classify::ClassifyResponse for ClassifyResponse {
    type Class = Class;
    type Error = h2::Error;
    type ClassifyEos = ClassifyEos;

    fn start<B>(self, rsp: &http::Response<B>) -> classify::ClassOrEos<Class, ClassifyEos> {
        for class in self.classes.as_ref() {
            if class.is_match(rsp) {
                let result = if class.is_failure() {
                    SuccessOrFailure::Failure
                } else {
                    SuccessOrFailure::Success
                };
                let c = Class::Http(result, rsp.status());
                return classify::ClassOrEos::Class(c);
            }
        }

        classify::ClassOrEos::Eos(ClassifyEos {
            status: rsp.status(),
        })
    }

    fn error(self, err: &h2::Error) -> Self::Class {
        Class::Stream(SuccessOrFailure::Failure, format!("{}", err))
    }
}

impl classify::ClassifyEos for ClassifyEos {
    type Class = Class;
    type Error = h2::Error;

    fn eos(self, trailers: Option<&http::HeaderMap>) -> Self::Class {
        if let Some(ref trailers) = trailers {
            let mut grpc_status = trailers
                .get("grpc-status")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u32>().ok());
            if let Some(grpc_status) = grpc_status.take() {
                return if grpc_status == 0 {
                    Class::Grpc(SuccessOrFailure::Success, grpc_status)
                } else {
                    Class::Grpc(SuccessOrFailure::Failure, grpc_status)
                };
            }
        }

        let result = if self.status.is_server_error() {
            SuccessOrFailure::Failure
        } else {
            SuccessOrFailure::Success
        };
        Class::Http(result, self.status)
    }

    fn error(self, err: &h2::Error) -> Self::Class {
        Class::Stream(SuccessOrFailure::Failure, format!("{}", err))
    }
}
