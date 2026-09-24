//! Shared application state: the frontend and the actors each hold a clone.
//!
//! Readers always see a consistent snapshot. Each field has at most one actor
//! that writes it; the intent handler may write any field.

use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::AppState;

/// Shared, lock-protected [`AppState`]. Cloning shares the same state.
#[derive(Debug, Clone, Default)]
pub struct State {
    inner: Arc<RwLock<AppState>>,
}

impl State {
    /// Share `data` behind a new lock.
    pub fn new(data: AppState) -> Self {
        Self {
            inner: Arc::new(RwLock::new(data)),
        }
    }

    /// Read the state. A writer that panicked doesn't lock readers out.
    pub fn read(&self) -> RwLockReadGuard<'_, AppState> {
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Change the state. A writer that panicked doesn't lock later writers out.
    pub fn write(&self) -> RwLockWriteGuard<'_, AppState> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Wakes the frontend loop after an actor changed [`AppState`], so the next
/// frame shows it.
pub type Wake = Arc<dyn Fn() + Send + Sync>;
