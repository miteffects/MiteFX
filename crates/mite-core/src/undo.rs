//! Undo and redo steps (BLUEPRINT §7.2): the states before each edit, as many as `depth`.
//! What a state holds is the plugin's business (the UI's session keeps parameters, snapshots and
//! the current preset's name).

#[derive(Debug, Clone)]
pub struct History<S> {
    undo: Vec<S>,
    redo: Vec<S>,
    depth: usize,
}

impl<S> History<S> {
    pub fn new(depth: usize) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), depth: depth.max(1) }
    }

    /// An edit happened: `before` is the state it left. Forgets what could be redone and the
    /// oldest step past the depth.
    pub fn push(&mut self, before: S) {
        self.undo.push(before);
        if self.undo.len() > self.depth {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The state to go back to, with `now` kept for redo.
    pub fn undo(&mut self, now: S) -> Option<S> {
        let state = self.undo.pop()?;
        self.redo.push(now);
        Some(state)
    }

    /// The state to go forward to, with `now` kept for undo.
    pub fn redo(&mut self, now: S) -> Option<S> {
        let state = self.redo.pop()?;
        self.undo.push(now);
        Some(state)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_the_steps_and_a_new_edit_drops_the_redo() {
        let mut h = History::new(2);
        h.push(1);
        h.push(2);
        h.push(3); // past the depth: 1 is forgotten
        assert_eq!(h.undo(4), Some(3));
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), None);
        assert_eq!(h.redo(2), Some(3));
        h.push(9);
        assert!(!h.can_redo());
        assert!(h.can_undo());
    }
}
