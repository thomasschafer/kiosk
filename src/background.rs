//! Work that carries on after the picker has told the app what it opened, such as typing
//! `[on_open]` commands, so the popup can close without waiting for it. The process waits for it before it
//! exits.

use std::{sync::Mutex, thread::JoinHandle};

pub struct Background {
    mode: Mode,
}

enum Mode {
    /// Each piece of work runs on a thread of its own.
    Threads(Mutex<Vec<JoinHandle<()>>>),
    /// Work runs only when waited for, so tests can see what happened before it.
    #[cfg(test)]
    Deferred(Mutex<Vec<Box<dyn FnOnce() + Send>>>),
}

impl Background {
    pub fn threads() -> Self {
        Self {
            mode: Mode::Threads(Mutex::new(Vec::new())),
        }
    }

    #[cfg(test)]
    pub fn deferred() -> Self {
        Self {
            mode: Mode::Deferred(Mutex::new(Vec::new())),
        }
    }

    pub fn spawn(&self, work: impl FnOnce() + Send + 'static) {
        match &self.mode {
            Mode::Threads(handles) => handles
                .lock()
                .expect("background work lock")
                .push(std::thread::spawn(work)),
            #[cfg(test)]
            Mode::Deferred(pending) => pending
                .lock()
                .expect("background work lock")
                .push(Box::new(work)),
        }
    }

    /// Waits for all the work given so far, running it here if it was deferred. A panic in the
    /// work is raised here.
    pub fn wait(&self) {
        match &self.mode {
            Mode::Threads(handles) => {
                let handles = std::mem::take(&mut *handles.lock().expect("background work lock"));
                for handle in handles {
                    if let Err(panic) = handle.join() {
                        std::panic::resume_unwind(panic);
                    }
                }
            }
            #[cfg(test)]
            Mode::Deferred(pending) => {
                let pending = std::mem::take(&mut *pending.lock().expect("background work lock"));
                for work in pending {
                    work();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[test]
    fn waiting_returns_once_threaded_work_is_done() {
        let background = Background::threads();
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        background.spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::SeqCst);
        });

        background.wait();

        assert!(done.load(Ordering::SeqCst));
    }

    #[test]
    fn deferred_work_runs_only_when_waited_for() {
        let background = Background::deferred();
        let done = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&done);
        background.spawn(move || flag.store(true, Ordering::SeqCst));
        assert!(!done.load(Ordering::SeqCst));

        background.wait();

        assert!(done.load(Ordering::SeqCst));
    }
}
