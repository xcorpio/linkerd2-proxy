use futures::{Async, Future, Poll, Stream};
use regex::Regex;
use std::fmt;
use std::time::Duration;
use tokio_timer::{clock, Delay};
use tower_grpc as grpc;
use tower_h2::{Body, BoxBody, Data, HttpService};

use api::destination as api;

use control::NameNormalizer;
use proxy::http::profiles;
use transport::DnsNameAndPort;

#[derive(Clone, Debug)]
pub struct Client<T, N> {
    service: Option<T>,
    normalizer: N,
    backoff: Duration,
}

pub struct Rx<T: HttpService> {
    dst: String,
    backoff: Duration,
    service: Option<T>,
    state: State<T>,
}

enum State<T: HttpService> {
    Disconnected,
    Backoff(Delay),
    Waiting(grpc::client::server_streaming::ResponseFuture<api::DestinationProfile, T::Future>),
    Streaming(grpc::Streaming<api::DestinationProfile, T::ResponseBody>),
}

// === impl Client ===

impl<T, N> Client<T, N>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
    T::Error: fmt::Debug,
    N: NameNormalizer,
{
    pub fn new(service: Option<T>, backoff: Duration, normalizer: N) -> Self {
        Self { service, backoff, normalizer }
    }
}

impl<T, N> profiles::GetRoutes for Client<T, N>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
    T::Error: fmt::Debug,
    N: NameNormalizer,
{
    type Stream = Rx<T>;

    fn get_routes(&self, dst: &DnsNameAndPort) -> Option<Self::Stream> {
        let fqa = self.normalizer.normalize(dst)?;
        Some(Rx {
            dst: fqa.without_trailing_dot().to_owned(),
            state: State::Disconnected,
            service: self.service.clone(),
            backoff: self.backoff,
        })
    }
}

// === impl Rx ===

impl<T> Stream for Rx<T>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
    T::Error: fmt::Debug,
{
    type Item = Vec<(profiles::RequestMatch, profiles::Route)>;
    type Error = profiles::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let service = match self.service {
            Some(ref s) => s,
            None => return Ok(Async::Ready(Some(Vec::new()))),
        };

        loop {
            self.state = match self.state {
                State::Disconnected => {
                    let mut client = api::client::Destination::new(service.clone());
                    let req = api::GetDestination {
                        scheme: "http".to_owned(),
                        path: self.dst.clone(),
                    };
                    debug!("disconnected; getting profile: {:?}", req);
                    let rspf = client.get_profile(grpc::Request::new(req));
                    State::Waiting(rspf)
                }
                State::Waiting(ref mut f) => match f.poll() {
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Ok(Async::Ready(rsp)) => {
                        debug!("response received");
                        State::Streaming(rsp.into_inner())
                    }
                    Err(e) => {
                        warn!("error fetching profile for {}: {:?}", self.dst, e);
                        State::Backoff(Delay::new(clock::now() + self.backoff))
                    }
                },
                State::Streaming(ref mut s) => match s.poll() {
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Ok(Async::Ready(Some(profile))) => {
                        debug!("profile received: {:?}", profile);
                        let routes = Self::convert_routes(profile);
                        return Ok(Async::Ready(Some(routes)));
                    }
                    Ok(Async::Ready(None)) => {
                        debug!("profile stream ended");
                        State::Backoff(Delay::new(clock::now() + self.backoff))
                    }
                    Err(e) => {
                        warn!("profile stream failed: {:?}", e);
                        State::Backoff(Delay::new(clock::now() + self.backoff))
                    }
                },
                State::Backoff(ref mut f) => match f.poll() {
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Err(_) | Ok(Async::Ready(())) => State::Disconnected,
                },
            };
        }
    }
}

impl<T: HttpService> Rx<T> {
    fn convert_routes(
        mut profile: api::DestinationProfile,
    ) -> Vec<(profiles::RequestMatch, profiles::Route)> {
        let mut routes = Vec::with_capacity(profile.routes.len());

        for r in profile.routes.drain(..) {
            if let Some(r) = Self::convert_route(r) {
                routes.push(r);
            }
        }

        routes
    }

    fn convert_route(orig: api::Route) -> Option<(profiles::RequestMatch, profiles::Route)> {
        let req_match = orig.condition.and_then(Self::convert_req_match)?;
        let route = profiles::Route::new(orig.metrics_labels.into_iter());
        Some((req_match, route))
    }

    fn convert_req_match(orig: api::RequestMatch) -> Option<profiles::RequestMatch> {
        let m = match orig.match_? {
            api::request_match::Match::All(mut ms) => {
                let ms = ms.matches.drain(..).filter_map(Self::convert_req_match);
                profiles::RequestMatch::All(ms.collect())
            }
            api::request_match::Match::Any(mut ms) => {
                let ms = ms.matches.drain(..).filter_map(Self::convert_req_match);
                profiles::RequestMatch::Any(ms.collect())
            }
            api::request_match::Match::Not(m) => {
                let m = Self::convert_req_match(*m)?;
                profiles::RequestMatch::Not(Box::new(m))
            }
            api::request_match::Match::Path(api::PathMatch { regex }) => {
                let re = Regex::new(&regex).ok()?;
                profiles::RequestMatch::Path(re)
            }
            api::request_match::Match::Method(mm) => {
                let m = mm.type_.and_then(|m| m.try_as_http().ok())?;
                profiles::RequestMatch::Method(m)
            }
        };

        Some(m)
    }
}
