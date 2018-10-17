use futures::{Async, Future, Poll, Stream};
use indexmap::IndexMap;
use regex::Regex;
use std::collections::HashMap;
use std::iter::FromIterator;
use tower_grpc as grpc;
use tower_h2::{Body, BoxBody, Data, HttpService};

use api::destination as api;

use proxy::http::profiles;

impl<T> profiles::GetRoutes for Option<T>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
{
    type Stream = Rx<T>;

    fn get_routes(&self, path: String) -> Self::Stream {
       Rx {
            state: State::Disconnected,
            service: self.clone(),
            dst: api::GetDestination {
                scheme: "".to_owned(),
                path,
            },
        }
    }
}

pub struct Rx<T: HttpService> {
    dst: api::GetDestination,
    service: Option<T>,
    state: State<T>,
}

enum State<T: HttpService> {
    Disconnected,
    Waiting(grpc::client::server_streaming::ResponseFuture<api::DestinationProfile, T::Future>),
    Streaming(grpc::Streaming<api::DestinationProfile, T::ResponseBody>),
}

impl<T> Stream for Rx<T>
where
    T: HttpService<RequestBody = BoxBody> + Clone,
    T::ResponseBody: Body<Data = Data>,
{
    type Item = Vec<(profiles::RequestMatch, profiles::Route)>;
    type Error = profiles::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.service {
            None => Ok(Async::Ready(Some(Vec::new()))),
            Some(ref service) => loop {
                self.state = match self.state {
                    State::Disconnected => {
                        let mut client = api::client::Destination::new(service.clone());
                        let rspf = client.get_profile(grpc::Request::new(self.dst.clone()));
                        State::Waiting(rspf)
                    }
                    State::Waiting(ref mut f) => match f.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Ok(Async::Ready(s)) => State::Streaming(s.into_inner()),
                        Err(_) => State::Disconnected,
                    },
                    State::Streaming(ref mut stream) => match stream.poll() {
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Ok(Async::Ready(Some(p))) => {
                            return Ok(Async::Ready(Some(Self::convert_routes(p))));
                        }
                        Ok(Async::Ready(None)) | Err(_) => State::Disconnected,

                    },
                };
            }
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

        let labels = Self::convert_labels(orig.metrics_labels);
        let route = profiles::Route { labels };

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

    fn convert_labels(labels: HashMap<String, String>) -> IndexMap<String, String> {
        let mut pairs = labels.into_iter().collect::<Vec<(String, String)>>();
        pairs.sort_by(|(k0, _), (k1, _)| k0.cmp(k1));
        IndexMap::from_iter(pairs)
    }
}
