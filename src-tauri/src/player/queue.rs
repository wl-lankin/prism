//! The play queue: an ordered list of track ids and the position in it. Pure data, no I/O.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    ids: Vec<String>,
    index: usize,
}

impl Queue {
    pub fn new() -> Queue {
        Queue::default()
    }

    /// Replaces the queue; `start` is clamped into range.
    pub fn set(&mut self, ids: Vec<String>, start: usize) {
        self.index = start.min(ids.len().saturating_sub(1));
        self.ids = ids;
    }

    /// Adds `ids` at the end, keeping the position.
    pub fn extend(&mut self, ids: Vec<String>) {
        self.ids.extend(ids);
    }

    /// Moves to `index`. False (and no move) when it is out of range.
    pub fn jump(&mut self, index: usize) -> bool {
        if index >= self.ids.len() {
            return false;
        }
        self.index = index;
        true
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn current(&self) -> Option<&str> {
        self.get(self.index)
    }

    /// The index after the current one, if there is one.
    pub fn next_index(&self) -> Option<usize> {
        let next = self.index + 1;
        (next < self.ids.len()).then_some(next)
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.ids.get(index).map(String::as_str)
    }

    /// Moves to the next track. False (and no move) at the end of the queue.
    pub fn advance(&mut self) -> bool {
        match self.next_index() {
            Some(next) => {
                self.index = next;
                true
            }
            None => false,
        }
    }

    /// Moves to the previous track. False (and no move) at the start of the queue.
    pub fn back(&mut self) -> bool {
        if self.index == 0 || self.ids.is_empty() {
            return false;
        }
        self.index -= 1;
        true
    }

    /// Goes back to the first track, keeping the ids.
    pub fn rewind(&mut self) {
        self.index = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(ids: &[&str], start: usize) -> Queue {
        let mut queue = Queue::new();
        queue.set(ids.iter().map(|id| id.to_string()).collect(), start);
        queue
    }

    #[test]
    fn empty_queue_has_no_current_track() {
        let mut queue = Queue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.current(), None);
        assert_eq!(queue.next_index(), None);
        assert!(!queue.advance());
        assert!(!queue.back());
    }

    #[test]
    fn set_clamps_the_start_index() {
        let queue = queue(&["a", "b", "c"], 7);
        assert_eq!(queue.index(), 2);
        assert_eq!(queue.current(), Some("c"));
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn advance_and_back_stop_at_the_ends() {
        let mut queue = queue(&["a", "b"], 0);
        assert_eq!(queue.next_index(), Some(1));
        assert!(queue.advance());
        assert_eq!(queue.current(), Some("b"));
        assert_eq!(queue.next_index(), None);
        assert!(!queue.advance());
        assert_eq!(queue.index(), 1);
        assert!(queue.back());
        assert!(!queue.back());
        assert_eq!(queue.current(), Some("a"));
    }

    #[test]
    fn queue_extend_and_jump() {
        let mut queue = queue(&["a", "b"], 1);
        queue.extend(vec!["c".to_string(), "d".to_string()]);
        assert_eq!(queue.ids(), ["a", "b", "c", "d"]);
        assert_eq!(queue.index(), 1, "extending keeps the position");
        assert_eq!(queue.next_index(), Some(2));
        assert!(queue.jump(3));
        assert_eq!(queue.current(), Some("d"));
        assert!(!queue.jump(4), "out of range");
        assert_eq!(queue.index(), 3, "a failed jump does not move");
        assert!(queue.jump(0));
        assert_eq!(queue.current(), Some("a"));

        let mut empty = Queue::new();
        assert!(!empty.jump(0));
        empty.extend(vec!["x".to_string()]);
        assert_eq!(empty.current(), Some("x"));
    }

    #[test]
    fn rewind_keeps_the_ids() {
        let mut queue = queue(&["a", "b", "c"], 2);
        queue.rewind();
        assert_eq!(queue.index(), 0);
        assert_eq!(queue.get(2), Some("c"));
        assert_eq!(queue.get(3), None);
    }
}
