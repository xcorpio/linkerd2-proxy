use http;

/// Determines how a request's response should be classified.
pub trait Classify {
    type Class;
    type Error;

    type ClassifyEos: ClassifyEos<Class = Self::Class, Error = Self::Error>;

    /// Classifies responses.
    ///
    /// Instances are intended to be used as an `http::Extension` that may be
    /// cloned to inner stack layers. Cloned instances are **not** intended to
    /// share state. Each clone should maintain its own internal state.
    type ClassifyResponse: ClassifyResponse<
            Class = Self::Class,
            Error = Self::Error,
            ClassifyEos = Self::ClassifyEos,
        >
        + Clone
        + Send
        + Sync
        + 'static;

    fn classify<B>(&self, req: &http::Request<B>) -> Self::ClassifyResponse;
}

/// Classifies a single response.
pub trait ClassifyResponse {
    /// A response classification.
    type Class;
    type Error;
    type ClassifyEos: ClassifyEos<Class = Self::Class, Error = Self::Error>;

    /// Update the classifier with the response headers.
    ///
    /// If this is enough data to classify a response, a classification may be
    /// returned. Implementations should expect that `end` or `error` may be
    /// called even when a class is returned.
    fn start<B>(self, headers: &http::Response<B>) -> ClassOrEos<Self::Class, Self::ClassifyEos>;

    /// Update the classifier with an underlying error.
    ///
    /// Because errors indicate an end-of-stream, a classification must be
    /// returned.
    fn error(self, error: &Self::Error) -> Self::Class;
}

/// Classifies a single response end
pub trait ClassifyEos {
    /// A response classification.
    type Class;
    type Error;

    /// Update the classifier with an EOS.
    ///
    /// Because trailers indicate an EOS, a classification must be returned.
    ///
    /// This is expected to be called only once.
    fn eos(self, trailers: Option<&http::HeaderMap>) -> Self::Class;

    /// Update the classifier with an underlying error.
    ///
    /// Because errors indicate an end-of-stream, a classification must be
    /// returned.
    ///
    /// This is expected to be called only once.
    fn error(self, error: &Self::Error) -> Self::Class;
}


pub enum ClassOrEos<C, E> {
    Class(C),
    Eos(E),
}
