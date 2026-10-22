//! The play queue: an ordered list of track ids and the position in it. Pure data, no I/O.
//!
//! The list is the play order. Shuffled, every entry keeps its place in the original order
//! (its rank), so turning shuffle off restores that order and plays on after the current
//! track's original place.

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    id: String,
    /// Unique for the queue's lifetime: follows the entry through moves, removals and
    /// shuffles (the same id may be queued twice).
    uid: u64,
    /// The entry's place in the original (unshuffled) order. Equal to its index while not
    /// shuffled.
    rank: u64,
}

#[derive(Debug, Clone)]
pub struct Queue {
    entries: Vec<Entry>,
    index: usize,
    shuffled: bool,
    next_uid: u64,
    rng: StdRng,
}

impl Default for Queue {
    fn default() -> Self {
        Queue::new()
    }
}

impl Queue {
    pub fn new() -> Queue {
        Queue::from_rng(StdRng::from_os_rng())
    }

    /// A queue whose shuffles are repeatable (tests).
    pub fn with_seed(seed: u64) -> Queue {
        Queue::from_rng(StdRng::seed_from_u64(seed))
    }

    fn from_rng(rng: StdRng) -> Queue {
        Queue {
            entries: Vec::new(),
            index: 0,
            shuffled: false,
            next_uid: 0,
            rng,
        }
    }

    fn entry(&mut self, id: String, rank: u64) -> Entry {
        let uid = self.next_uid;
        self.next_uid += 1;
        Entry { id, uid, rank }
    }

    /// Replaces the queue; `start` is clamped into range. With `shuffle` the start track
    /// comes first and the others follow in random order.
    pub fn set(&mut self, ids: Vec<String>, start: usize, shuffle: bool) {
        let start = start.min(ids.len().saturating_sub(1));
        let entries: Vec<Entry> = ids
            .into_iter()
            .enumerate()
            .map(|(rank, id)| self.entry(id, rank as u64))
            .collect();
        self.entries = entries;
        self.shuffled = shuffle;
        self.index = start;
        if shuffle && !self.entries.is_empty() {
            let first = self.entries.remove(start);
            self.entries.insert(0, first);
            self.index = 0;
            self.entries[1..].shuffle(&mut self.rng);
        }
    }

    /// Adds `ids` at the end, keeping the position. Shuffled, they go to random places in
    /// the upcoming part (and at the end of the original order).
    pub fn extend(&mut self, ids: Vec<String>) {
        let first_rank = self.entries.iter().map(|e| e.rank + 1).max().unwrap_or(0);
        for (rank, id) in (first_rank..).zip(ids) {
            let entry = self.entry(id, rank);
            if self.shuffled && !self.entries.is_empty() {
                let at = self.rng.random_range(self.index + 1..=self.entries.len());
                self.entries.insert(at, entry);
            } else {
                self.entries.push(entry);
            }
        }
    }

    /// Moves the entry at `from` to `to`; the current index stays on its track. False (and
    /// no change) when either is out of range or they are equal.
    pub fn move_entry(&mut self, from: usize, to: usize) -> bool {
        let len = self.entries.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        self.index = moved_index(self.index, from, to);
        self.renumber();
        true
    }

    /// Removes the entry at `index`. The current index stays on its track; when that one is
    /// removed, it stays put (on the track that followed), clamped to the last entry.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.entries.remove(index);
        if index < self.index {
            self.index -= 1;
        }
        self.index = self.index.min(self.entries.len().saturating_sub(1));
        self.renumber();
        true
    }

    /// On: the entries after the current one are shuffled. Off: the original order comes
    /// back and the current track keeps playing at its original place.
    pub fn set_shuffled(&mut self, on: bool) {
        if on == self.shuffled {
            return;
        }
        self.shuffled = on;
        if on {
            if self.index + 1 < self.entries.len() {
                let from = self.index + 1;
                self.entries[from..].shuffle(&mut self.rng);
            }
        } else {
            let current = self.uid(self.index);
            self.entries.sort_by_key(|entry| entry.rank);
            if let Some(index) = current.and_then(|uid| self.position_of(uid)) {
                self.index = index;
            }
            self.renumber();
        }
    }

    pub fn is_shuffled(&self) -> bool {
        self.shuffled
    }

    /// Not shuffled, the play order is the original order.
    fn renumber(&mut self) {
        if self.shuffled {
            return;
        }
        for (rank, entry) in self.entries.iter_mut().enumerate() {
            entry.rank = rank as u64;
        }
    }

    /// Moves to `index`. False (and no move) when it is out of range.
    pub fn jump(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        self.index = index;
        true
    }

    /// The ids in play order.
    pub fn ids(&self) -> Vec<String> {
        self.entries.iter().map(|entry| entry.id.clone()).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
        (next < self.entries.len()).then_some(next)
    }

    /// The index after the current one, going round to the first after the last.
    pub fn wrap_next(&self) -> Option<usize> {
        self.next_index()
            .or_else(|| (!self.entries.is_empty()).then_some(0))
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(|entry| entry.id.as_str())
    }

    /// The entry at `index`'s unique id.
    pub fn uid(&self, index: usize) -> Option<u64> {
        self.entries.get(index).map(|entry| entry.uid)
    }

    /// Where the entry `uid` is now.
    pub fn position_of(&self, uid: u64) -> Option<usize> {
        self.entries.iter().position(|entry| entry.uid == uid)
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
        if self.index == 0 || self.entries.is_empty() {
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

/// Where the entry at `index` is after the entry at `from` moved to `to`.
fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < index && to >= index {
        index - 1
    } else if from > index && to <= index {
        index + 1
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(ids: &[&str], start: usize) -> Queue {
        let mut queue = Queue::new();
        queue.set(ids.iter().map(|id| id.to_string()).collect(), start, false);
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

    fn seeded(ids: &[&str], start: usize, shuffle: bool, seed: u64) -> Queue {
        let mut queue = Queue::with_seed(seed);
        queue.set(
            ids.iter().map(|id| id.to_string()).collect(),
            start,
            shuffle,
        );
        queue
    }

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    const TEN: [&str; 10] = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];

    #[test]
    fn move_keeps_the_current_index_on_its_track() {
        let mut queue = queue(&["a", "b", "c", "d", "e"], 2);
        // An upcoming track moves within the upcoming part.
        assert!(queue.move_entry(4, 3));
        assert_eq!(queue.ids(), strings(&["a", "b", "c", "e", "d"]));
        assert_eq!(queue.current(), Some("c"));
        // From before the current track to after it: the current one moves up.
        assert!(queue.move_entry(0, 4));
        assert_eq!(queue.ids(), strings(&["b", "c", "e", "d", "a"]));
        assert_eq!(queue.index(), 1);
        assert_eq!(queue.current(), Some("c"));
        // From after it to before it: the current one moves down.
        assert!(queue.move_entry(3, 0));
        assert_eq!(queue.ids(), strings(&["d", "b", "c", "e", "a"]));
        assert_eq!(queue.index(), 2);
        assert_eq!(queue.current(), Some("c"));
        // The current track itself.
        assert!(queue.move_entry(2, 4));
        assert_eq!(queue.index(), 4);
        assert_eq!(queue.current(), Some("c"));
        // Out of range or onto itself: nothing.
        assert!(!queue.move_entry(5, 0));
        assert!(!queue.move_entry(0, 5));
        assert!(!queue.move_entry(1, 1));
        assert_eq!(queue.ids(), strings(&["d", "b", "e", "a", "c"]));
    }

    #[test]
    fn remove_keeps_the_current_index_on_its_track() {
        let mut queue = queue(&["a", "b", "c", "d"], 2);
        assert!(queue.remove(3));
        assert_eq!(queue.ids(), strings(&["a", "b", "c"]));
        assert_eq!(queue.current(), Some("c"));
        assert!(queue.remove(0));
        assert_eq!(queue.index(), 1);
        assert_eq!(queue.current(), Some("c"));
        assert!(!queue.remove(2), "out of range");
        // The current one: the index stays, on the track that followed (clamped at the end).
        assert!(queue.remove(1));
        assert_eq!(queue.ids(), strings(&["b"]));
        assert_eq!(queue.index(), 0);
        assert!(queue.remove(0));
        assert!(queue.is_empty());
        assert_eq!(queue.index(), 0);
        assert_eq!(queue.current(), None);
    }

    #[test]
    fn uids_follow_entries_through_moves_and_removals() {
        let mut queue = queue(&["a", "b", "c", "a"], 0);
        let first_a = queue.uid(0).unwrap();
        let second_a = queue.uid(3).unwrap();
        assert_ne!(first_a, second_a, "the same id twice is two entries");
        queue.move_entry(3, 1);
        queue.remove(0);
        assert_eq!(queue.position_of(second_a), Some(0));
        assert_eq!(queue.position_of(first_a), None);
        assert_eq!(queue.uid(9), None);
    }

    #[test]
    fn shuffle_keeps_the_played_part_and_the_current_track() {
        let mut queue = seeded(&TEN, 3, false, 7);
        queue.set_shuffled(true);
        assert!(queue.is_shuffled());
        assert_eq!(queue.index(), 3);
        assert_eq!(&queue.ids()[..4], strings(&["a", "b", "c", "d"]).as_slice());
        let mut upcoming = queue.ids()[4..].to_vec();
        assert_ne!(
            upcoming,
            strings(&TEN[4..]),
            "the upcoming part was shuffled"
        );
        upcoming.sort();
        assert_eq!(upcoming, strings(&TEN[4..]), "nothing lost or duplicated");
    }

    #[test]
    fn shuffle_off_restores_the_order_after_the_current_track() {
        let mut queue = seeded(&TEN, 3, false, 11);
        queue.set_shuffled(true);
        // Play on into the shuffled part.
        queue.advance();
        queue.advance();
        let current = queue.current().unwrap().to_string();
        queue.set_shuffled(false);
        assert!(!queue.is_shuffled());
        assert_eq!(queue.ids(), strings(&TEN), "the original order is back");
        assert_eq!(
            queue.current(),
            Some(current.as_str()),
            "the same track plays on"
        );
        assert_eq!(
            queue.index(),
            TEN.iter().position(|id| *id == current).unwrap()
        );
        // Turning it off again changes nothing.
        queue.set_shuffled(false);
        assert_eq!(queue.ids(), strings(&TEN));
    }

    #[test]
    fn set_while_shuffled_starts_with_the_chosen_track() {
        let mut queue = seeded(&TEN, 4, true, 3);
        assert_eq!(queue.index(), 0);
        assert_eq!(queue.current(), Some("e"));
        let mut all = queue.ids();
        all.sort();
        assert_eq!(all, strings(&TEN));
        queue.set_shuffled(false);
        assert_eq!(queue.ids(), strings(&TEN));
        assert_eq!(queue.index(), 4, "continues after the chosen track's place");
    }

    #[test]
    fn appends_while_shuffled_land_in_the_upcoming_part() {
        let mut queue = seeded(&TEN, 2, true, 5);
        queue.advance();
        queue.advance();
        let played = queue.ids()[..=queue.index()].to_vec();
        queue.extend(strings(&["x", "y", "z", "w"]));
        assert_eq!(queue.len(), 14);
        assert_eq!(
            &queue.ids()[..=queue.index()],
            played.as_slice(),
            "history untouched"
        );
        let tail: Vec<String> = queue.ids()[TEN.len()..].to_vec();
        assert_ne!(
            tail,
            strings(&["x", "y", "z", "w"]),
            "not just put at the end"
        );
        queue.set_shuffled(false);
        let mut expected = strings(&TEN);
        expected.extend(strings(&["x", "y", "z", "w"]));
        assert_eq!(queue.ids(), expected, "appended after the original order");
    }

    #[test]
    fn moves_and_removals_while_shuffled_survive_shuffle_off() {
        let mut queue = seeded(&TEN, 0, true, 9);
        let current = queue.current().unwrap().to_string();
        let gone = queue.get(5).unwrap().to_string();
        assert!(queue.remove(5));
        assert!(queue.move_entry(1, 7));
        queue.set_shuffled(false);
        let expected: Vec<String> = strings(&TEN).into_iter().filter(|id| *id != gone).collect();
        assert_eq!(
            queue.ids(),
            expected,
            "a move in the shuffled order is not an original move"
        );
        assert_eq!(queue.current(), Some(current.as_str()));
    }

    #[test]
    fn moves_while_not_shuffled_are_the_new_original_order() {
        let mut queue = queue(&["a", "b", "c", "d"], 0);
        queue.move_entry(3, 1);
        queue.set_shuffled(true);
        queue.set_shuffled(false);
        assert_eq!(queue.ids(), strings(&["a", "d", "b", "c"]));
    }

    #[test]
    fn wrap_next_goes_round() {
        let mut queue = queue(&["a", "b"], 0);
        assert_eq!(queue.wrap_next(), Some(1));
        queue.advance();
        assert_eq!(queue.wrap_next(), Some(0));
        assert_eq!(Queue::new().wrap_next(), None);
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
