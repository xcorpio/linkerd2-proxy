use futures::{Async, Future, Poll};
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
    let (reg_tx, reg_rx) = mpsc::channel(REGISTER_BUFFER_CAPACITY);
    let (tap_tx, tap_rx) = mpsc::channel(TAP_BUFFER_CAPACITY);

    let daemon = Daemon {
        reg_rx,
        regs: VecDeque::default(),

        tap_rx,
        taps: VecDeque::default(),
    };

    (daemon, Register(reg_tx), Subscribe(tap_tx))
}

#[derive(Debug)]
pub struct Daemon<T> {
    reg_rx: mpsc::Receiver<mpsc::Sender<Weak<T>>>,
    regs: VecDeque<mpsc::Sender<Weak<T>>>,

    tap_rx: mpsc::Receiver<Weak<T>>,
    taps: VecDeque<Weak<T>>,
}

#[derive(Clone, Debug)]
pub struct Register<T>(mpsc::Sender<mpsc::Sender<Weak<T>>>);

#[derive(Debug)]
pub struct Subscribe<T>(mpsc::Sender<Weak<T>>);

impl<T: Tap> Future for Daemon<T> {
    type Item = ();
    type Error = Never;

    fn poll(&mut self) -> Poll<(), Never> {
        while let Ok(Async::Ready(tap)) = self.tap_rx.poll_ready() {
            for tx in &mut self.regs {
                let _ = tx.try_send(tap.clone());
            }
            self.taps.push_back(tap);
        }

        while let Ok(Async::Ready(mut tx)) = self.reg_rx.poll_ready() {
            for tap in &self.taps {
                let _ = tx.try_send(tap.clone());
            }
            self.regs.push_back(tx);
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

impl<T: Tap> Subscribe<T> for Subscribe<T> {
    fn subscribe(&mut self, tap: Weak<T>) {
        let _ = self.0.try_send(tap);
    }
}
