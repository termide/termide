//! Priority queue for file operations.

use std::collections::VecDeque;

use crate::types::{OperationId, OperationPriority, OperationRequest};

/// Entry in the operation queue.
#[derive(Debug)]
pub struct QueuedOperation {
    /// Operation ID.
    pub id: OperationId,
    /// Operation request.
    pub request: OperationRequest,
    /// Priority level.
    pub priority: OperationPriority,
    /// Timestamp when queued (for FIFO ordering within priority).
    pub queued_at: std::time::Instant,
}

impl QueuedOperation {
    /// Create a new queued operation.
    pub fn new(id: OperationId, request: OperationRequest) -> Self {
        let priority = request.priority;
        Self {
            id,
            request,
            priority,
            queued_at: std::time::Instant::now(),
        }
    }
}

/// Priority queue for operations.
///
/// Operations are processed by priority (higher first), then by arrival time (FIFO).
#[derive(Debug)]
pub struct OperationQueue {
    /// Queued operations, sorted by priority.
    operations: VecDeque<QueuedOperation>,
    /// Maximum queue size (0 = unlimited).
    max_size: usize,
}

impl Default for OperationQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationQueue {
    /// Create a new operation queue.
    pub fn new() -> Self {
        Self {
            operations: VecDeque::new(),
            max_size: 0,
        }
    }

    /// Create a queue with a maximum size.
    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            operations: VecDeque::new(),
            max_size,
        }
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Get the number of queued operations.
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Check if the queue is full.
    pub fn is_full(&self) -> bool {
        self.max_size > 0 && self.operations.len() >= self.max_size
    }

    /// Enqueue an operation.
    ///
    /// Returns `true` if the operation was added, `false` if the queue is full.
    pub fn enqueue(&mut self, operation: QueuedOperation) -> bool {
        if self.is_full() {
            return false;
        }

        // Find insertion point (maintain priority order, FIFO within same priority)
        let insert_pos = self
            .operations
            .iter()
            .position(|op| op.priority < operation.priority)
            .unwrap_or(self.operations.len());

        self.operations.insert(insert_pos, operation);
        true
    }

    /// Dequeue the next operation (highest priority, oldest).
    pub fn dequeue(&mut self) -> Option<QueuedOperation> {
        self.operations.pop_front()
    }

    /// Peek at the next operation without removing it.
    pub fn peek(&self) -> Option<&QueuedOperation> {
        self.operations.front()
    }

    /// Remove an operation by ID.
    pub fn remove(&mut self, id: OperationId) -> Option<QueuedOperation> {
        if let Some(pos) = self.operations.iter().position(|op| op.id == id) {
            self.operations.remove(pos)
        } else {
            None
        }
    }

    /// Get an operation by ID.
    pub fn get(&self, id: OperationId) -> Option<&QueuedOperation> {
        self.operations.iter().find(|op| op.id == id)
    }

    /// Change priority of a queued operation.
    pub fn set_priority(&mut self, id: OperationId, priority: OperationPriority) -> bool {
        if let Some(mut op) = self.remove(id) {
            op.priority = priority;
            op.request.priority = priority;
            self.enqueue(op)
        } else {
            false
        }
    }

    /// Get all queued operation IDs.
    pub fn ids(&self) -> Vec<OperationId> {
        self.operations.iter().map(|op| op.id).collect()
    }

    /// Clear all queued operations.
    pub fn clear(&mut self) {
        self.operations.clear();
    }

    /// Iterate over queued operations.
    pub fn iter(&self) -> impl Iterator<Item = &QueuedOperation> {
        self.operations.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OperationPath, OperationType};

    fn make_request(priority: OperationPriority) -> OperationRequest {
        OperationRequest {
            op_type: OperationType::Copy,
            sources: vec![OperationPath::local("/test")],
            destination: Some(OperationPath::local("/dest")),
            priority,
            is_move: false,
            conflict_mode: crate::types::ConflictMode::Ask,
            archive_passwords: Default::default(),
            compression: Default::default(),
        }
    }

    #[test]
    fn test_priority_ordering() {
        let mut queue = OperationQueue::new();

        // Add in mixed priority order
        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Low),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(2),
            make_request(OperationPriority::High),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(3),
            make_request(OperationPriority::Normal),
        ));

        // Should come out in priority order
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(2)); // High
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(3)); // Normal
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(1)); // Low
    }

    #[test]
    fn test_fifo_within_priority() {
        let mut queue = OperationQueue::new();

        // Add multiple with same priority
        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Normal),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(2),
            make_request(OperationPriority::Normal),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(3),
            make_request(OperationPriority::Normal),
        ));

        // Should come out in FIFO order
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(1));
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(2));
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(3));
    }

    #[test]
    fn test_max_size() {
        let mut queue = OperationQueue::with_max_size(2);

        assert!(queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Normal),
        )));
        assert!(queue.enqueue(QueuedOperation::new(
            OperationId::new(2),
            make_request(OperationPriority::Normal),
        )));
        assert!(!queue.enqueue(QueuedOperation::new(
            OperationId::new(3),
            make_request(OperationPriority::Normal),
        )));

        assert!(queue.is_full());
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn test_remove_by_id() {
        let mut queue = OperationQueue::new();
        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Normal),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(2),
            make_request(OperationPriority::Normal),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(3),
            make_request(OperationPriority::Normal),
        ));

        // Remove middle one
        let removed = queue.remove(OperationId::new(2));
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, OperationId::new(2));
        assert_eq!(queue.len(), 2);

        // Verify order: 1, 3
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(1));
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(3));
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut queue = OperationQueue::new();
        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Normal),
        ));
        let removed = queue.remove(OperationId::new(99));
        assert!(removed.is_none());
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_peek() {
        let mut queue = OperationQueue::new();
        assert!(queue.peek().is_none());

        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Normal),
        ));
        assert_eq!(queue.peek().unwrap().id, OperationId::new(1));
        // peek doesn't remove
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut queue = OperationQueue::new();
        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Normal),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(2),
            make_request(OperationPriority::Normal),
        ));
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_set_priority() {
        let mut queue = OperationQueue::new();
        queue.enqueue(QueuedOperation::new(
            OperationId::new(1),
            make_request(OperationPriority::Low),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(2),
            make_request(OperationPriority::Low),
        ));

        // Promote op 2 to high priority
        assert!(queue.set_priority(OperationId::new(2), OperationPriority::High));

        // Op 2 should now come first
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(2));
        assert_eq!(queue.dequeue().unwrap().id, OperationId::new(1));
    }

    #[test]
    fn test_ids() {
        let mut queue = OperationQueue::new();
        queue.enqueue(QueuedOperation::new(
            OperationId::new(10),
            make_request(OperationPriority::Normal),
        ));
        queue.enqueue(QueuedOperation::new(
            OperationId::new(20),
            make_request(OperationPriority::Normal),
        ));
        let ids = queue.ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&OperationId::new(10)));
        assert!(ids.contains(&OperationId::new(20)));
    }

    #[test]
    fn test_unlimited_queue() {
        let mut queue = OperationQueue::new(); // max_size = 0 = unlimited
        assert!(!queue.is_full());
        for i in 0..100 {
            assert!(queue.enqueue(QueuedOperation::new(
                OperationId::new(i),
                make_request(OperationPriority::Normal),
            )));
        }
        assert_eq!(queue.len(), 100);
    }
}
