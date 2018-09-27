use h2;
use http;

use proxy::http::classify;

#[derive(Clone, Debug)]
pub struct Classify;

#[derive(Clone, Debug)]
pub struct ClassifyResponse;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Class;

impl classify::Classify for Classify {
    type Class = Class;
    type Error = h2::Error;
    type ClassifyResponse = ClassifyResponse;

    fn classify<B>(&self, _: &http::Request<B>) -> Self::ClassifyResponse {
        ClassifyResponse
    }
}

impl classify::ClassifyResponse for ClassifyResponse {
    type Class = Class;
    type Error = h2::Error;

    fn start<B>(&mut self, _: &http::Response<B>) -> Option<Self::Class> {
        None
    }

    fn eos(&mut self, _: Option<&http::HeaderMap>) -> Self::Class {
        Class
    }

    fn error(&mut self, _: &Self::Error) -> Self::Class {
        Class
    }
}

