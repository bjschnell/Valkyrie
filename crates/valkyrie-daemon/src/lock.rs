//! Locking that survives a panic elsewhere (backlog #228).

use std::sync::{Mutex, MutexGuard, PoisonError};

/// `Mutex::lock` that takes the data even when another thread panicked while
/// holding it. A poisoned lock would otherwise make every later request touching
/// it panic too, and one bad moment would wedge the whole daemon; the data a
/// panic leaves behind (a screen, a status, a list) is still worth serving.
pub(crate) trait Locked<T: ?Sized> {
    fn locked(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> Locked<T> for Mutex<T> {
    fn locked(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
