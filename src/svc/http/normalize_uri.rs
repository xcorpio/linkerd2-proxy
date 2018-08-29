
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
    pub fn new(inner: S, was_absolute_form: bool) -> Self {
        Self { inner, was_absolute_form }
    }
}

impl<N, B> tower::NewService for NormalizeUri<N>
where
    N: tower::NewService<
        Request=http::Request<B>,
        Response=HttpResponse,
    >,
    N::Service: tower::Service<
        Request=http::Request<B>,
        Response=HttpResponse,
    >,
    NormalizeUri<N::Service>: tower::Service,
    B: tower_h2::Body,
{
    type Request = <Self::Service as tower::Service>::Request;
    type Response = <Self::Service as tower::Service>::Response;
    type Error = <Self::Service as tower::Service>::Error;
    type Service = NormalizeUri<S::Service>;
    type InitError = S::InitError;
    type Future = future::Map<
        S::Future,
        fn(S::Service) -> NormalizeUri<S::Service>
    >;

    fn new_service(&self) -> Self::Future {
        let s = self.inner.new_service();
        // This weird dance is so that the closure doesn't have to
        // capture `self` and can just be a `fn` (so the `Map`)
        // can be returned unboxed.
        if self.was_absolute_form {
            s.map(|inner| NormalizeUri::new(inner, true))
        } else {
            s.map(|inner| NormalizeUri::new(inner, false))
        }
    }
}

impl<S, B> tower::Service for NormalizeUri<S>
where
    S: tower::Service<
        Request=http::Request<B>,
        Response=HttpResponse,
    >,
    B: tower_h2::Body,
{
    type Request = S::Request;
    type Response = HttpResponse;
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
