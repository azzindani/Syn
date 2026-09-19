//! Run queue: pause/resume/cancel semantics for live orchestration.
//! Pause freezes dispatch and keeps files open; resume revalidates every
//! queued handle against the registry before continuing (human may have
//! edited, closed, or renamed files mid-pause). Cancel drops the queue.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum QueueState {
    #[default]
    Idle,
    Running,
    Paused,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedOp {
    pub handle: String,
    pub summary: String,
}

#[derive(Debug, Default)]
pub struct RunQueue {
    state: QueueState,
    items: Vec<QueuedOp>,
    done: Vec<QueuedOp>,
}

impl RunQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, op: QueuedOp) {
        if self.state == QueueState::Cancelled {
            return;
        }
        self.items.push(op);
        if self.state == QueueState::Idle {
            self.state = QueueState::Running;
        }
    }

    pub fn pause(&mut self) {
        if self.state == QueueState::Running {
            self.state = QueueState::Paused;
        }
    }

    /// Freeze from any non-terminal state (doom-loop auto-pause after drain).
    pub fn freeze(&mut self) {
        if self.state != QueueState::Cancelled {
            self.state = QueueState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == QueueState::Paused {
            self.state = QueueState::Running;
        }
    }

    pub fn cancel(&mut self) {
        self.items.clear();
        self.state = QueueState::Cancelled;
    }

    pub fn state(&self) -> &QueueState {
        &self.state
    }

    pub fn pending(&self) -> usize {
        self.items.len()
    }

    /// Next op only while Running. Paused/Cancelled/Idle yield nothing.
    pub fn pop_next(&mut self) -> Option<QueuedOp> {
        if self.state != QueueState::Running {
            return None;
        }
        let op = self.items.first().cloned()?;
        self.items.remove(0);
        self.done.push(op.clone());
        if self.items.is_empty() {
            self.state = QueueState::Idle;
        }
        Some(op)
    }

    /// Revalidate queued handles against a live registry snapshot.
    /// Returns handles that vanished mid-pause (caller must surface them).
    pub fn revalidate(&mut self, registry: &[String]) -> Vec<String> {
        let missing: Vec<String> =
            self.items.iter().map(|o| o.handle.clone()).filter(|h| !registry.contains(h)).collect();
        self.items.retain(|o| registry.contains(&o.handle));
        if self.items.is_empty() && self.state == QueueState::Running {
            self.state = QueueState::Idle;
        }
        missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(h: &str) -> QueuedOp {
        QueuedOp { handle: h.into(), summary: "write".into() }
    }

    #[test]
    fn pause_freezes_resume_continues() {
        let mut q = RunQueue::new();
        q.enqueue(op("a"));
        q.enqueue(op("b"));
        assert!(q.pop_next().is_some());
        q.pause();
        assert_eq!(q.state(), &QueueState::Paused);
        assert!(q.pop_next().is_none());
        q.resume();
        assert!(q.pop_next().is_some());
        assert!(q.pop_next().is_none());
        assert_eq!(q.state(), &QueueState::Idle);
    }

    #[test]
    fn cancel_drops_and_locks() {
        let mut q = RunQueue::new();
        q.enqueue(op("a"));
        q.cancel();
        assert_eq!(q.state(), &QueueState::Cancelled);
        assert!(q.pop_next().is_none());
        q.enqueue(op("b"));
        assert_eq!(q.pending(), 0);
    }

    #[test]
    fn revalidate_prunes_vanished_handles() {
        let mut q = RunQueue::new();
        q.enqueue(op("a"));
        q.enqueue(op("gone"));
        let missing = q.revalidate(&["a".to_string()]);
        assert_eq!(missing, vec!["gone".to_string()]);
        assert_eq!(q.pending(), 1);
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn fifo_order_preserved() {
        let mut q = RunQueue::new();
        for h in ["first", "second", "third"] {
            q.enqueue(QueuedOp { handle: h.into(), summary: format!("job {h}") });
        }
        let order: Vec<_> = [q.pop_next(), q.pop_next(), q.pop_next()]
            .into_iter()
            .map(|o| o.unwrap().handle)
            .collect();
        assert_eq!(order, vec!["first", "second", "third"]);
    }

    #[test]
    fn freeze_pauses_from_any_live_state() {
        let mut q = RunQueue::new();
        q.freeze(); // idle -> paused, nothing lost
        assert_eq!(q.state(), &QueueState::Paused);
        q.enqueue(QueuedOp { handle: "a".into(), summary: "x".into() });
        assert!(q.pop_next().is_none());
        q.resume();
        assert!(q.pop_next().is_some());
    }

    #[test]
    fn freeze_never_uncancels() {
        let mut q = RunQueue::new();
        q.cancel();
        q.freeze();
        assert_eq!(q.state(), &QueueState::Cancelled);
    }

    #[test]
    fn enqueue_while_paused_stays_paused() {
        let mut q = RunQueue::new();
        q.enqueue(QueuedOp { handle: "a".into(), summary: "x".into() });
        q.pause();
        q.enqueue(QueuedOp { handle: "b".into(), summary: "y".into() });
        assert_eq!(q.pending(), 2);
        assert_eq!(q.state(), &QueueState::Paused);
    }
}
