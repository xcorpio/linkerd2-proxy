use std::fmt::Write;
use std::mem;
use std::sync::Arc;

use bytes::BytesMut;
use http;
use http::header::{HOST, UPGRADE};
use http::uri::{Authority, Parts, Scheme, Uri};

use ctx::transport::{Server as ServerCtx};

/// Tries to make sure the `Uri` of the request is in a form needed by
/// hyper's Client.
pub fn normalize_our_view_of_uri<B>(req: &mut http::Request<B>) {
    debug_assert!(
        req.uri().scheme_part().is_none(),
        "normalize_uri shouldn't be called with absolute URIs: {:?}",
        req.uri()
    );

    // try to parse the Host header
    if let Some(auth) = authority_from_host(&req) {
        set_authority(req.uri_mut(), auth);
        return;
    }

    // last resort is to use the so_original_dst
    let orig_dst = req.extensions()
        .get::<Arc<ServerCtx>>()
        .and_then(|ctx| ctx.orig_dst_if_not_local());
    if let Some(orig_dst) = orig_dst {
        let mut bytes = BytesMut::with_capacity(31);
        write!(&mut bytes, "{}", orig_dst)
            .expect("socket address display is under 31 bytes");
        let bytes = bytes.freeze();
        let auth = Authority::from_shared(bytes)
            .expect("socket address is valid authority");
        set_authority(req.uri_mut(), auth);
    }
}

/// Convert any URI into its origin-form (relative path part only).
pub fn set_origin_form(uri: &mut Uri) {
    let mut parts = mem::replace(uri, Uri::default()).into_parts();
    parts.scheme = None;
    parts.authority = None;
    *uri = Uri::from_parts(parts)
        .expect("path only is valid origin-form uri")
}

/// Returns an Authority from a request's Host header.
pub fn authority_from_host<B>(req: &http::Request<B>) -> Option<Authority> {
    req.headers().get(HOST)
        .and_then(|host| {
             host.to_str().ok()
                .and_then(|s| {
                    if s.is_empty() {
                        None
                    } else {
                        s.parse::<Authority>().ok()
                    }
                })
        })
}

fn set_authority(uri: &mut http::Uri, auth: Authority) {
    let mut parts = Parts::from(mem::replace(uri, Uri::default()));

    parts.authority = Some(auth);

    // If this was an origin-form target (path only),
    // then we can't *only* set the authority, as that's
    // an illegal target (such as `example.com/docs`).
    //
    // But don't set a scheme if this was authority-form (CONNECT),
    // since that would change its meaning (like `https://example.com`).
    if parts.path_and_query.is_some() {
        parts.scheme = Some(Scheme::HTTP);
    }

    let new = Uri::from_parts(parts)
        .expect("absolute uri");

    *uri = new;
}

/// Checks requests to determine if they want to perform an HTTP upgrade.
pub fn wants_upgrade<B>(req: &http::Request<B>) -> bool {
    // HTTP upgrades were added in 1.1, not 1.0.
    if req.version() != http::Version::HTTP_11 {
        return false;
    }

    if let Some(upgrade) = req.headers().get(UPGRADE) {
        // If an `h2` upgrade over HTTP/1.1 were to go by the proxy,
        // and it succeeded, there would an h2 connection, but it would
        // be opaque-to-the-proxy, acting as just a TCP proxy.
        //
        // A user wouldn't be able to see any usual HTTP telemetry about
        // requests going over that connection. Instead of that confusion,
        // the proxy strips h2 upgrade headers.
        //
        // Eventually, the proxy will support h2 upgrades directly.
        return upgrade != "h2c";
    }

    // HTTP/1.1 CONNECT requests are just like upgrades!
    req.method() == &http::Method::CONNECT
}

/// Returns if the request target is in `absolute-form`.
///
/// This is `absolute-form`: `https://example.com/docs`
///
/// This is not:
///
/// - `/docs`
/// - `example.com`
pub fn is_absolute_form(uri: &Uri) -> bool {
    // It's sufficient just to check for a scheme, since in HTTP1,
    // it's required in absolute-form, and `http::Uri` doesn't
    // allow URIs with the other parts missing when the scheme is set.
    debug_assert!(
        uri.scheme_part().is_none() ||
        (
            uri.authority_part().is_some() &&
            uri.path_and_query().is_some()
        ),
        "is_absolute_form http::Uri invariants: {:?}",
        uri
    );

    uri.scheme_part().is_some()
}

/// Returns if the request target is in `origin-form`.
///
/// This is `origin-form`: `example.com`
fn is_origin_form(uri: &Uri) -> bool {
    uri.scheme_part().is_none() &&
        uri.path_and_query().is_none()
}
