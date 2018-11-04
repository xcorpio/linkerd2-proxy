use http;

pub struct Layer;

pub struct Stack<M> { inner: M }

pub struct Service<M, S> {
    stack: M,
    inner: S,
}

// ===== impl Settings =====

impl Settings {
    pub fn detect<B>(req: &http::Request<B>) -> Self {
        if req.version() == http::Version::HTTP_2 {
            return Settings::Http2;
        }

        let was_absolute_form = super::h1::is_absolute_form(req.uri());
        trace!(
            "Settings::detect(); req.uri='{:?}'; was_absolute_form={:?};",
            req.uri(), was_absolute_form
        );

        let is_h1_upgrade = super::h1::wants_upgrade(req);

        Settings::Http1 {
            is_h1_upgrade,
            was_absolute_form,
        }
    }

    /// Returns true if the request was originally received in absolute form.
    pub fn was_absolute_form(&self) -> bool {
        match self {
            &Settings::Http1 { was_absolute_form, .. } => was_absolute_form,
            _ => false,
        }
    }

    pub fn can_reuse_clients(&self) -> bool {
        match *self {
            Settings::Http2 | Settings::Http1 { host: Host::Authority(_), .. } => true,
            _ => false,
        }
    }

    pub fn is_h1_upgrade(&self) -> bool {
        match *self {
            Settings::Http1 { is_h1_upgrade: true, .. } => true,
            _ => false,
        }
    }

    pub fn is_http2(&self) -> bool {
        match *self {
            Settings::Http2 => true,
            _ => false,
        }
    }
}
