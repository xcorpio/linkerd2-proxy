use h2;
use http;
use std::sync::Arc;

use proxy::http::{classify, profiles};

#[derive(Clone, Debug)]
pub struct Request {
    classes: Arc<Vec<profiles::ResponseClass>>,
}

#[derive(Clone, Debug)]
pub enum Response {
    Grpc,
    Http {
        classes: Arc<Vec<profiles::ResponseClass>>,
    },
}

#[derive(Clone, Debug)]
pub enum Eos {
    Http(HttpEos),
    Grpc(GrpcEos),
}

#[derive(Clone, Debug)]
pub enum HttpEos {
    FromProfile(Class),
    Pending(http::StatusCode),
}

#[derive(Clone, Debug)]
pub enum GrpcEos {
    NoBody(Class),
    Open,
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

// === impl Request ===

impl Request {
    pub fn new(classes: Vec<profiles::ResponseClass>) -> Self {
        Self {
            classes: Arc::new(classes),
        }
    }
}

impl classify::Classify for Request {
    type Class = Class;
    type Error = h2::Error;
    type ClassifyResponse = Response;
    type ClassifyEos = Eos;

    fn classify<B>(&self, req: &http::Request<B>) -> Self::ClassifyResponse {
        // Determine if the request is a gRPC request by checking the content-type.
        if let Some(ref ct) = req
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
        {
            if ct.starts_with("application/grpc+") {
                return Response::Grpc;
            }
        }

        Response::Http {
            classes: self.classes.clone(),
        }
    }
}

// === impl Response ===

impl Response {
    fn match_class<B>(
        rsp: &http::Response<B>,
        classes: &Vec<profiles::ResponseClass>,
    ) -> Option<Class> {
        for class in classes {
            if class.is_match(rsp) {
                let result = if class.is_failure() {
                    SuccessOrFailure::Failure
                } else {
                    SuccessOrFailure::Success
                };
                return Some(Class::Http(result, rsp.status()));
            }
        }

        None
    }
}

impl classify::ClassifyResponse for Response {
    type Class = Class;
    type Error = h2::Error;
    type ClassifyEos = Eos;

    fn start<B>(self, rsp: &http::Response<B>) -> (Eos, Option<Class>) {
        match self {
            Response::Grpc => match grpc_class(rsp.headers()) {
                None => (Eos::Grpc(GrpcEos::Open), None),
                Some(class) => {
                    let eos = Eos::Grpc(GrpcEos::NoBody(class.clone()));
                    (eos, Some(class))
                }
            },
            Response::Http { ref classes } => {
                match Self::match_class(rsp, classes.as_ref()) {
                    None => (Eos::Http(HttpEos::Pending(rsp.status())), None),
                    Some(class) => {
                        let eos = Eos::Http(HttpEos::FromProfile(class.clone()));
                        (eos, Some(class))
                    }
                }
            }
        }
    }

    fn error(self, err: &h2::Error) -> Self::Class {
        Class::Stream(SuccessOrFailure::Failure, format!("{}", err))
    }
}

impl Default for Response {
    fn default() -> Self {
        // By default, simply perform HTTP classification.
        Response::Http { classes: Arc::new(Vec::new()) }
    }
}

// === impl Eos ===

impl classify::ClassifyEos for Eos {
    type Class = Class;
    type Error = h2::Error;

    fn eos(self, trailers: Option<&http::HeaderMap>) -> Self::Class {
        match self {
            Eos::Http(http) => http.eos(trailers),
            Eos::Grpc(grpc) => grpc.eos(trailers),
        }
    }

    fn error(self, err: &h2::Error) -> Self::Class {
        match self {
            Eos::Http(http) => http.error(err),
            Eos::Grpc(grpc) => grpc.error(err),
        }
    }
}

impl classify::ClassifyEos for HttpEos {
    type Class = Class;
    type Error = h2::Error;

    fn eos(self, _: Option<&http::HeaderMap>) -> Self::Class {
        match self {
            HttpEos::FromProfile(class) => class,
            HttpEos::Pending(status) if status.is_server_error() => {
                Class::Http(SuccessOrFailure::Failure, status)
            }
            HttpEos::Pending(status) => {
                Class::Http(SuccessOrFailure::Success, status)
            }
        }
    }

    fn error(self, err: &h2::Error) -> Self::Class {
        Class::Stream(SuccessOrFailure::Failure, format!("{}", err))
    }
}

impl classify::ClassifyEos for GrpcEos {
    type Class = Class;
    type Error = h2::Error;

    fn eos(self, trailers: Option<&http::HeaderMap>) -> Self::Class {
        match self {
            GrpcEos::NoBody(class) => class,
            GrpcEos::Open => trailers
                .and_then(grpc_class)
                .unwrap_or_else(|| Class::Grpc(SuccessOrFailure::Success, 0)),
        }
    }

    fn error(self, err: &h2::Error) -> Self::Class {
        // Ignore the original classification when an error is encountered.
        Class::Stream(SuccessOrFailure::Failure, format!("{}", err))
    }
}

fn grpc_class(headers: &http::HeaderMap) -> Option<Class> {
    headers
        .get("grpc-status")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok())
        .map(|grpc_status| {
            if grpc_status == 0 {
                Class::Grpc(SuccessOrFailure::Success, grpc_status)
            } else {
                Class::Grpc(SuccessOrFailure::Failure, grpc_status)
            }
        })
}

#[cfg(test)]
mod tests {
    use http::{HeaderMap, Response, StatusCode};

    use super::{Class, SuccessOrFailure};
    use proxy::http::classify::{ClassifyEos as _CE, ClassifyResponse as _CR};

    #[test]
    fn http_response_status_ok() {
        let rsp = Response::builder().status(StatusCode::OK).body(()).unwrap();
        let crsp = super::Response::default();
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class, None);

        let class = ceos.eos(None);
        assert_eq!(
            class,
            Class::Http(SuccessOrFailure::Success, StatusCode::OK)
        );
    }

    #[test]
    fn http_response_status_bad_request() {
        let rsp = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(())
            .unwrap();
        let crsp = super::Response::default();
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class, None);

        let class = ceos.eos(None);
        assert_eq!(
            class,
            Class::Http(SuccessOrFailure::Success, StatusCode::BAD_REQUEST)
        );
    }

    #[test]
    fn http_response_status_server_error() {
        let rsp = Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(())
            .unwrap();
        let crsp = super::Response::default();
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class, None);

        let class = ceos.eos(None);
        assert_eq!(
            class,
            Class::Http(SuccessOrFailure::Failure, StatusCode::INTERNAL_SERVER_ERROR)
        );
    }

    #[test]
    fn grpc_response_header_ok() {
        let rsp = Response::builder()
            .header("grpc-status", "0")
            .status(StatusCode::OK)
            .body(())
            .unwrap();
        let crsp = super::Response::Grpc;
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class, Some(Class::Grpc(SuccessOrFailure::Success, 0)));

        let class = ceos.eos(None);
        assert_eq!(class, Class::Grpc(SuccessOrFailure::Success, 0));
    }

    #[test]
    fn grpc_response_header_error() {
        let rsp = Response::builder()
            .header("grpc-status", "2")
            .status(StatusCode::OK)
            .body(())
            .unwrap();
        let crsp = super::Response::Grpc;
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class, Some(Class::Grpc(SuccessOrFailure::Failure, 2)));

        let class = ceos.eos(None);
        assert_eq!(class, Class::Grpc(SuccessOrFailure::Failure, 2));
    }

    #[test]
    fn grpc_response_trailer_ok() {
        let rsp = Response::builder().status(StatusCode::OK).body(()).unwrap();
        let crsp = super::Response::Grpc;
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class.as_ref(), None);

        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", 0.into());

        let class = ceos.eos(Some(&trailers));
        assert_eq!(class, Class::Grpc(SuccessOrFailure::Success, 0));
    }

    #[test]
    fn grpc_response_trailer_error() {
        let rsp = Response::builder().status(StatusCode::OK).body(()).unwrap();
        let crsp = super::Response::Grpc;
        let (ceos, class) = crsp.start(&rsp);
        assert_eq!(class.as_ref(), None);

        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", 3.into());

        let class = ceos.eos(Some(&trailers));
        assert_eq!(class, Class::Grpc(SuccessOrFailure::Failure, 3));
    }
}
