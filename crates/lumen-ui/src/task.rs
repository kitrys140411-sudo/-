//! Background work that wakes the UI.
//!
//! `view` runs on the UI thread and must stay quick, so anything slow
//! (reading a big directory, decoding an image, talking to D-Bus, polling
//! `/proc`) runs on another thread and hands its results back:
//!
//! - [`Task`]: one result, from a closure run on a new thread
//!   (`ui.spawn(|| load(path))`). The view polls it with [`Task::take`].
//! - [`channel`]: a stream of values from a long-running thread (a sampler,
//!   a D-Bus signal listener). The view drains the [`Receiver`].
//!
//! Sending wakes the window, so the next `view` runs right away (the UI never
//! polls on a timer for results). Values cross threads, so they must be
//! `Send`: send decoded [`crate::Pixels`] and make the [`crate::Image`] in
//! the view.

use std::sync::mpsc;

use crate::window::Waker;

/// The sending half of a [`channel`]: cloneable, usable from any thread.
pub struct Sender<T> {
    tx: mpsc::Sender<T>,
    waker: Waker,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        Sender { tx: self.tx.clone(), waker: self.waker.clone() }
    }
}

impl<T> Sender<T> {
    /// Send a value and wake the UI. False when the receiving side is gone
    /// (the window closed, or the app dropped the receiver): a worker
    /// thread should stop then.
    pub fn send(&self, value: T) -> bool {
        let ok = self.tx.send(value).is_ok();
        if ok {
            self.waker.wake();
        }
        ok
    }
}

/// The receiving half of a [`channel`], kept in the app state.
pub struct Receiver<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> Receiver<T> {
    /// Every value sent since the last call, oldest first (never blocks).
    pub fn drain(&self) -> Vec<T> {
        self.rx.try_iter().collect()
    }

    /// The newest value sent since the last call (older ones are dropped).
    pub fn latest(&self) -> Option<T> {
        self.rx.try_iter().last()
    }

    /// The next value if one is waiting.
    pub fn try_recv(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

/// A channel whose sends wake the UI (see the module docs).
pub fn channel<T>(waker: Waker) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Sender { tx, waker }, Receiver { rx })
}

/// A closure running on its own thread; its result is picked up by the view.
pub struct Task<T> {
    rx: Receiver<T>,
    result: Option<T>,
    done: bool,
}

impl<T: Send + 'static> Task<T> {
    /// Run `f` on a new thread; `waker` is woken when it returns.
    pub fn spawn(waker: Waker, f: impl FnOnce() -> T + Send + 'static) -> Task<T> {
        let (tx, rx) = channel(waker);
        std::thread::spawn(move || {
            tx.send(f());
        });
        Task { rx, result: None, done: false }
    }
}

impl<T> Task<T> {
    fn poll(&mut self) {
        if !self.done {
            if let Some(v) = self.rx.try_recv() {
                self.result = Some(v);
                self.done = true;
            }
        }
    }

    /// The result, once (None while running, and after it was taken).
    pub fn take(&mut self) -> Option<T> {
        self.poll();
        self.result.take()
    }

    /// The result, if the task finished (without taking it).
    pub fn result(&mut self) -> Option<&T> {
        self.poll();
        self.result.as_ref()
    }

    /// Still running.
    pub fn is_running(&mut self) -> bool {
        self.poll();
        !self.done
    }
}

impl crate::Ui {
    /// Run `f` on a background thread (see [`Task`]).
    pub fn spawn<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Task<T> {
        Task::spawn(self.waker(), f)
    }

    /// A channel whose sends wake this window (see [`channel`]).
    pub fn channel<T>(&self) -> (Sender<T>, Receiver<T>) {
        channel(self.waker())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_send<T: Send + Sync>() {}

    #[test]
    fn wakers_and_senders_cross_threads() {
        is_send::<Waker>();
        let (tx, rx) = channel::<u32>(Waker::none());
        let t = std::thread::spawn(move || {
            for i in 0..3 {
                assert!(tx.send(i));
            }
        });
        t.join().unwrap();
        assert_eq!(rx.drain(), vec![0, 1, 2]);
        assert_eq!(rx.latest(), None);
    }

    #[test]
    fn task_result_is_taken_once() {
        let mut task = Task::spawn(Waker::none(), || 6 * 7);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while task.is_running() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(task.result(), Some(&42));
        assert_eq!(task.take(), Some(42));
        assert_eq!(task.take(), None);
        assert!(!task.is_running());
    }

    #[test]
    fn senders_notice_a_dropped_receiver() {
        let (tx, rx) = channel::<u8>(Waker::none());
        drop(rx);
        assert!(!tx.send(1));
    }
}
