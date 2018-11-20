use futures::{future, Poll, Stream};
use futures_mpsc_lossy;
use http::HeaderMap;
use indexmap::IndexMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tower_grpc::{self as grpc, Response};

use super::Subscribe;
use api::tap::{observe_request, server, ObserveRequest, TapEvent};
use convert::*;

#[derive(Clone, Debug)]
pub struct Server<T: Subscribe> {
    subscribe: T,
}

pub struct ResponseStream {
    observation: Observation,
    remaining: usize,
}

impl<T: Subscribe> Server<T> {
    pub(super) fn new(subscribe: T) -> Self {
        Self { subscribe }
    }
}

impl<T: Subscribe> server::Tap for Server<T> {
    type ObserveStream = ResponseStream;
    type ObserveFuture = future::FutureResult<Response<Self::ObserveStream>, grpc::Error>;

    fn observe(&mut self, req: grpc::Request<ObserveRequest>) -> Self::ObserveFuture {
        let req = req.into_inner();

        let match_ = match Match::new(&req.match_match_) {
            Ok(m) => m,
            Err(_) => {
                return future::err(grpc::Error::Grpc(
                    grpc::Status::with_code(grpc::Code::InvalidArgument),
                    HeaderMap::new(),
                ))
            }
        };

        let tap = self.subscribe.subscribe(match_);
        future::ok(Response::new(ResponseStream {
            tap,
            remaining: req.limit as usize,
        }))
    }
}

impl<T: Subscribe> ResponseStream<T::Stream> {
    fn response(stream: T::Stream) -> Self {
        unimplemented!()
    }
}

impl Stream for ResponseStream {
    type Item = TapEvent;
    type Error = grpc::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        loop {
            if self.remaining == 0 && self.current.is_empty() {
                trace!("tap completed");
                return Ok(None.into());
            }

            let poll: Poll<Option<Event>, Self::Error> =
                self.rx.poll().or_else(|_| Ok(None.into()));

            trace!("polling; remaining={}; current={}", self.remaining, {
                use std::fmt::Write;
                let mut s = String::new();
                write!(s, "[").unwrap();
                for id in self.current.keys() {
                    write!(s, "{},", *id).unwrap();
                }
                write!(s, "]").unwrap();
                s
            });
            match try_ready!(poll) {
                Some(ev) => {
                    match ev {
                        Event::StreamRequestOpen(ref req) => {
                            if self.remaining == 0 {
                                trace!("exhausted; ignoring req={}", req.id);
                                continue;
                            }
                            trace!("insert req={}", req.id);
                            self.remaining -= 1;
                            let _ = self.current.insert(req.id, req.clone());
                        }
                        Event::StreamRequestFail(ref req, _) => {
                            trace!("fail req={}", req.id);
                            if self.current.remove(&req.id).is_none() {
                                warn!("did not exist req={}", req.id);
                                continue;
                            }
                        }
                        Event::StreamResponseOpen(ref rsp, _) => {
                            trace!("response req={}", rsp.request.id);
                            if !self.current.contains_key(&rsp.request.id) {
                                warn!("did not exist req={}", rsp.request.id);
                                continue;
                            }
                        }
                        Event::StreamResponseFail(ref rsp, _)
                        | Event::StreamResponseEnd(ref rsp, _) => {
                            trace!("end req={}", rsp.request.id);
                            if self.current.remove(&rsp.request.id).is_none() {
                                warn!("did not exist req={}", rsp.request.id);
                                continue;
                            }
                        }
                        ev => {
                            trace!("ignoring event: {:?}", ev);
                            continue;
                        }
                    }

                    trace!("emitting tap event: {:?}", ev);
                    if let Ok(te) = TapEvent::try_from(&ev) {
                        trace!("emitted tap event");
                        // TODO Do limit checks here.
                        return Ok(Some(te).into());
                    }
                }
                None => {
                    return Ok(None.into());
                }
            }
        }
    }
}

impl Drop for ResponseStream {
    fn drop(&mut self) {
        if let Ok(mut taps) = self.taps.lock() {
            let _ = (*taps).remove(self.tap_id);
        }
    }
}
