//! Telling whether a shared value is the one seen last time.

use std::sync::{Arc, Weak};

/// Remembers the last `Arc` a cache was built from, to tell a new one apart.
///
/// Comparing raw addresses is not enough: once the old value is freed, a new
/// one can be allocated at its address and pass for it, and the cache keeps
/// the old settings. The `Weak` held here keeps the allocation (not the
/// value) alive, so no later `Arc` can share its address.
#[derive(Debug)]
pub struct SeenArc<T>(Weak<T>);

impl<T> Default for SeenArc<T> {
    fn default() -> Self {
        Self(Weak::new())
    }
}

impl<T> SeenArc<T> {
    /// `true`, remembering `current`, when it is not the `Arc` seen last.
    pub fn changed(&mut self, current: &Arc<T>) -> bool {
        if std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(current)) {
            return false;
        }
        self.0 = Arc::downgrade(current);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_value_is_told_apart_from_the_old_one_even_after_it_is_freed() {
        let mut seen = SeenArc::default();
        let first = Arc::new(1u64);
        assert!(seen.changed(&first));
        assert!(!seen.changed(&first));
        assert!(!seen.changed(&Arc::clone(&first)));
        drop(first);
        // Same size and alignment, so the allocator would reuse the slot.
        let second = Arc::new(2u64);
        assert!(seen.changed(&second));
        assert!(!seen.changed(&second));
    }
}
