use futures::{Async, Future, Poll, Stream};
use http;
use regex::Regex;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use tokio_timer::{clock, Delay};
use tower_grpc::{self as grpc, Body, BoxBody};
use tower_http::HttpService;
use tower_retry::budget::Budget;

use api::destination as api;

use proxy::http::profiles;
use NameAddr;

#[derive(Clone, Debug)]
pub struct Client<T> {
    service: Option<T>,
    backoff: Duration,
}

pub struct Rx<T>
where
    T: HttpService<BoxBody>,
    T::ResponseBody: Body,
{
    dst: String,
    backoff: Duration,
    service: Option<T>,
    state: State<T>,
}

enum State<T>
where
    T: HttpService<BoxBody>,
    T::ResponseBody: Body,
{
    Disconnected,
    Backoff(Delay),
    Waiting(grpc::client::server_streaming::ResponseFuture<api::DestinationProfile, T::Future>),
    Streaming(grpc::Streaming<api::DestinationProfile, T::ResponseBody>),
}

// === impl Client ===

impl<T> Client<T>
where
    T: HttpService<BoxBody> + Clone,
    T::ResponseBody: Body,
    T::Error: fmt::Debug,
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
    T: HttpService<BoxBody> + Clone,
    T::ResponseBody: Body,
    T::Error: fmt::Debug,
{
    type Stream = Rx<T>;

    fn get_routes(&self, dst: &NameAddr) -> Option<Self::Stream> {
        Some(Rx {
            dst: format!("{}", dst),
            state: State::Disconnected,
            service: self.service.clone(),
            backoff: self.backoff,
        })
    }
}

// === impl Rx ===

impl<T> Stream for Rx<T>
where
    T: HttpService<BoxBody> + Clone,
    T::ResponseBody: Body,
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
                        scheme: "k8s".to_owned(),
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
                        let retry_budget = profile.retry_budget.and_then(convert_retry_budget);
                        let default_retry_timeout = profile.default_retry_timeout.map(Into::into);
                        let routes = profile
                            .routes
                            .into_iter()
                            .filter_map(move |orig| {
                                convert_route(orig, retry_budget.as_ref(), default_retry_timeout)
                            });
                        return Ok(Async::Ready(Some(routes.collect())));
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

fn convert_route(orig: api::Route, retry_budget: Option<&Arc<Budget>>, default_retry_timeout: Option<Result<Duration, Duration>>) -> Option<(profiles::RequestMatch, profiles::Route)> {
    let req_match = orig.condition.and_then(convert_req_match)?;
    let rsp_classes = orig
        .response_classes
        .into_iter()
        .filter_map(convert_rsp_class)
        .collect();
    let mut route = profiles::Route::new(orig.metrics_labels.into_iter(), rsp_classes);
    if orig.is_retryable {
        set_route_retry(&mut route, orig.retry_timeout.map(Into::into).or(default_retry_timeout), retry_budget);
    }
    Some((req_match, route))
}

fn set_route_retry(route: &mut profiles::Route, retry_timeout: Option<Result<Duration, Duration>>, retry_budget: Option<&Arc<Budget>>) {
    match retry_timeout {
        Some(Ok(dur)) => route.set_retry_timeout(dur),
        Some(Err(_)) => {
            warn!("route retry_timeout is negative: {:?}", route);
            return;
        },
        None => {
            warn!("retry_timeout is missing: {:?}", route);
            return;
        },
    }

    if let Some(budget) = retry_budget {
        route.set_retry_budget(budget.clone());
    } else {
        warn!("route claims is_retryable, but missing retry_budget: {:?}", route);
        return;
    }

    route.set_is_retryable(true);
}

fn convert_req_match(orig: api::RequestMatch) -> Option<profiles::RequestMatch> {
    let m = match orig.match_? {
        api::request_match::Match::All(ms) => {
            let ms = ms.matches.into_iter().filter_map(convert_req_match);
            profiles::RequestMatch::All(ms.collect())
        }
        api::request_match::Match::Any(ms) => {
            let ms = ms.matches.into_iter().filter_map(convert_req_match);
            profiles::RequestMatch::Any(ms.collect())
        }
        api::request_match::Match::Not(m) => {
            let m = convert_req_match(*m)?;
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

fn convert_rsp_class(orig: api::ResponseClass) -> Option<profiles::ResponseClass> {
    let c = orig.condition.and_then(convert_rsp_match)?;
    Some(profiles::ResponseClass::new(orig.is_failure, c))
}

fn convert_rsp_match(orig: api::ResponseMatch) -> Option<profiles::ResponseMatch> {
    let m = match orig.match_? {
        api::response_match::Match::All(ms) => {
            let ms = ms
                .matches
                .into_iter()
                .filter_map(convert_rsp_match)
                .collect::<Vec<_>>();
            if ms.is_empty() {
                return None;
            }
            profiles::ResponseMatch::All(ms)
        }
        api::response_match::Match::Any(ms) => {
            let ms = ms
                .matches
                .into_iter()
                .filter_map(convert_rsp_match)
                .collect::<Vec<_>>();
            if ms.is_empty() {
                return None;
            }
            profiles::ResponseMatch::Any(ms)
        }
        api::response_match::Match::Not(m) => {
            let m = convert_rsp_match(*m)?;
            profiles::ResponseMatch::Not(Box::new(m))
        }
        api::response_match::Match::Status(range) => {
            let min = http::StatusCode::from_u16(range.min as u16).ok()?;
            let max = http::StatusCode::from_u16(range.max as u16).ok()?;
            profiles::ResponseMatch::Status { min, max }
        }
    };

    Some(m)
}

fn convert_retry_budget(orig: api::RetryBudget) -> Option<Arc<Budget>> {
    let min_retries = if orig.min_retries_per_second <= ::std::i32::MAX as u32 {
        orig.min_retries_per_second as isize
    } else {
        warn!("retry_budget min_retries_per_second overflow: {:?}", orig.min_retries_per_second);
        return None;
    };
    let retry_ratio = orig.retry_ratio;
    if retry_ratio > 1000.0 || retry_ratio <= 0.0 {
        warn!("retry_budget retry_ratio invalid: {:?}", retry_ratio);
        return None;
    }
    let ttl = match orig.ttl?.into() {
        Ok(dur) => {
            let secs = dur.as_secs();
            if secs > 60 || secs <= 0 {
                warn!("retry_budget ttl invalid: {:?}", dur);
                return None;
            }
            dur
        },
        Err(_) => return None,
    };

    Some(Arc::new(Budget::new(ttl, min_retries, retry_ratio)))
}
