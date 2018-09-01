use futures::{Future, Poll, future};
use http;

use svc::{Service, NewService, http::h1};

/// Rewrites HTTP/1.x requests so that their URIs are in a canonical form.
///
/// The following transformations are applied:
/// - If an absolute-form URI is received, it must replace
///   the host header (in accordance with RFC7230#section-5.4)
/// - If the request URI is not in absolute form, it is rewritten to contain
///   the authority given in the `Host:` header, or, failing that, from the
///   request's original destination according to `SO_ORIGINAL_DST`.
#[derive(Copy, Clone, Debug)]
pub struct NormalizeUri<T> {
    inner: T,
    was_absolute_form: bool,
}

// ===== impl NormalizeUri =====

impl<T> NormalizeUri<T> {
    pub fn new(inner: T, was_absolute_form: bool) -> Self {
        Self { inner, was_absolute_form }
    }
}

impl<N, A, B> NewService for NormalizeUri<N>
where
    N: NewService<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
    NormalizeUri<N::Service>: Service<
        Request = N::Request,
        Response = N::Response,
        Error = N::Error,
    >,
{
    type Request = <N as NewService>::Request;
    type Response = <N as NewService>::Response;
    type Error = <N as NewService>::Error;
    type Service = NormalizeUri<N::Service>;
    type InitError = N::InitError;
    type Future = future::Map<
        N::Future,
        fn(N::Service) -> NormalizeUri<N::Service>
    >;

    fn new_service(&self) -> Self::Future {
        let fut = self.inner.new_service();
        // This weird dance is so that the closure doesn't have to
        // capture `self` and can just be a `fn` (so the `Map`)
        // can be returned unboxed.
        if self.was_absolute_form {
            fut.map(|inner| NormalizeUri::new(inner, true))
        } else {
            fut.map(|inner| NormalizeUri::new(inner, false))
        }
    }
}

impl<S, A, B> Service for NormalizeUri<S>
where
    S: Service<
        Request = http::Request<A>,
        Response = http::Response<B>,
    >,
{
    type Request = S::Request;
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self) -> Poll<(), S::Error> {
        self.inner.poll_ready()
    }

    fn call(&mut self, mut request: S::Request) -> Self::Future {
        if request.version() != http::Version::HTTP_2 &&
            // Skip normalizing the URI if it was received in
            // absolute form.
            !self.was_absolute_form
        {
            h1::normalize_our_view_of_uri(&mut request);
        }
        self.inner.call(request)
    }
}
