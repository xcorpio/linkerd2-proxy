use futures::{Async, Future, Poll, Stream};
use regex::Regex;
use std::time::Duration;
use tokio_timer::{clock, Delay};
use tower_grpc as grpc;
use tower_h2::{Body, BoxBody, Data, HttpService};

use api::destination as api;

use proxy::http::profiles;

#[derive(Clone, Debug)]
pub struct Client<T> {
    service: Option<T>,
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

impl<T> Client<T>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
{
    pub fn new(service: Option<T>, backoff: Duration) -> Self {
        Self {
            service,
            backoff,
        }
    }
}

impl<T> profiles::GetRoutes for Client<T>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
{
    type Stream = Rx<T>;

    fn get_routes(&self, dst: String) -> Self::Stream {
        Rx {
            state: State::Disconnected,
            service: self.service.clone(),
            dst,
            backoff: self.backoff,
        }
    }
}

// === impl Rx ===

impl<T> Stream for Rx<T>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
{
    type Item = Vec<(profiles::RequestMatch, profiles::Route)>;
    type Error = profiles::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if let Some(ref service) = self.service {
            loop {
                self.state = match self.state {
                    State::Disconnected => {
                        let mut client = api::client::Destination::new(service.clone());
                        let rspf = client.get_profile(grpc::Request::new(api::GetDestination {
                            scheme: "http".to_owned(),
                            path: self.dst.clone(),
                        }));
                        State::Waiting(rspf)
                    }
                    State::Backoff(ref mut f) => match f.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(_) | Ok(Async::Ready(())) => State::Disconnected,
                    }
                   State::Waiting(ref mut f) => match f.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Ok(Async::Ready(rsp)) => State::Streaming(rsp.into_inner()),
                        Err(_) => State::Backoff(Delay::new(clock::now() + self.backoff)),
                    },
                    State::Streaming(ref mut s) => match s.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Ok(Async::Ready(Some(profile))) => {
                            let routes = Self::convert_routes(profile);
                            return Ok(Async::Ready(Some(routes)));
                        }
                        Ok(Async::Ready(None)) | Err(_) => {
                            State::Backoff(Delay::new(clock::now() + self.backoff))
                        }
                    },
                };
            }
        }

        Ok(Async::Ready(Some(Vec::new())))
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
