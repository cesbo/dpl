use std::sync::{
    Arc,
    atomic::{
        AtomicBool,
        Ordering,
    },
};

use super::DeployError;

pub struct BusyGuard(Arc<AtomicBool>);

impl BusyGuard {
    pub fn lock(busy: Arc<AtomicBool>) -> Result<Self, DeployError> {
        if busy.swap(true, Ordering::Acquire) {
            Err(DeployError::EntityBusy)
        } else {
            Ok(BusyGuard(busy))
        }
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
