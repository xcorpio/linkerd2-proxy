use futures::{Async, Future, Poll, Stream};
use futures::sync::mpsc;
use never::Never;
use std::collections::VecDeque;
use std::sync::Weak;

use super::Tap;

/// The number of pending registrations that may be buffered.
const REGISTER_BUFFER_CAPACITY: usize = 16;

/// The number of pending taps that may be buffered.
const TAP_BUFFER_CAPACITY: usize = 16;

/// The number of tap requests a given layer may buffer before consuming.
const REGISTER_TAPS_BUFFER_CAPACITY: usize = 16;

pub fn new<T>() -> (Daemon<T>, Register<T>, Subscribe<T>) {
    let (svc_tx, svc_rx) = mpsc::channel(REGISTER_BUFFER_CAPACITY);
    let (tap_tx, tap_rx) = mpsc::channel(TAP_BUFFER_CAPACITY);

    let daemon = Daemon {
        svc_rx,
        svcs: VecDeque::default(),

        tap_rx,
        taps: VecDeque::default(),
    };

    (daemon, Register(svc_tx), Subscribe(tap_tx))
}

#[must_use = "daemon must be polled"]
#[derive(Debug)]
pub struct Daemon<T> {
    svc_rx: mpsc::Receiver<mpsc::Sender<Weak<T>>>,
    svcs: VecDeque<mpsc::Sender<Weak<T>>>,

    tap_rx: mpsc::Receiver<Weak<T>>,
    taps: VecDeque<Weak<T>>,
}

#[derive(Clone, Debug)]
pub struct Register<T>(mpsc::Sender<mpsc::Sender<Weak<T>>>);

#[derive(Clone, Debug)]
pub struct Subscribe<T>(mpsc::Sender<Weak<T>>);

impl<T: Tap> Future for Daemon<T> {
    type Item = ();
    type Error = Never;

    fn poll(&mut self) -> Poll<(), Never> {
        // Drop taps that are no longer active (i.e. the response stream has
        // been droped).
        self.taps.retain(|t| t.upgrade().is_some());

        // Connect newly-created services to active taps.
        while let Ok(Async::Ready(Some(mut svc))) = self.svc_rx.poll() {
            // Notify the service of all active taps. If there's an error, the
            // registration is dropped.
            let mut is_ok = true;
            for tap in &self.taps {
                if is_ok {
                    is_ok = svc.try_send(tap.clone()).is_ok();
                }
            }

            if is_ok {
                self.svcs.push_back(svc);
            }
        }

        // Connect newly-created taps to existing services.
        while let Ok(Async::Ready(Some(tap))) = self.tap_rx.poll() {
            if tap.upgrade().is_none() {
                continue;
            }

            // Notify services of the new tap. If the tap can't be sent to a
            // given service, it's assumed that the service has been dropped, so
            // it is removed from the registry.
            self.svcs.retain(|s| s.try_send(tap.clone()).is_ok());

            self.taps.push_back(tap);
        }

        Ok(Async::NotReady)
    }
}

impl<T: Tap> super::Register for Register<T> {
    type Tap = T;
    type Taps = mpsc::Receiver<Weak<T>>;

    fn register(&mut self) -> Self::Taps {
        let (tx, rx) = mpsc::channel(REGISTER_TAPS_BUFFER_CAPACITY);
        let _ = self.0.try_send(tx);
        rx
    }
}

impl<T: Tap> super::Subscribe<T> for Subscribe<T> {
    fn subscribe(&mut self, tap: Weak<T>) {
        let _ = self.0.try_send(tap);
    }
}
