//! The Flow deck: deals a pool in cycles, weighted toward less recently played tracks, with
//! artists spaced apart.

use std::collections::{BTreeSet, HashMap, VecDeque};

use rand::distr::Open01;
use rand::rngs::StdRng;
use rand::Rng;

use super::Candidate;

const DAY_MS: i64 = 86_400_000;
/// Days since the last play stop counting after this.
const MAX_DAYS: i64 = 60;

/// Deals a pool in cycles: no repeats until the whole pool was dealt, each cycle weighted
/// toward less recently played tracks and spaced so the same artist (by name, ignoring case)
/// does not play twice in a row when avoidable (also across a cycle boundary).
pub struct Deck {
    pool: Vec<Candidate>,
    /// The pool's artists as small numbers, by pool index.
    artist_of: Vec<usize>,
    /// Each artist number's key (the name in lower case).
    artist_keys: Vec<String>,
    now_ms: i64,
    rng: StdRng,
    cycle: Cycle,
    last_artist: Option<usize>,
}

/// The rest of the current cycle, in key order, kept per artist: `heads` holds each artist's
/// next position, so "the first remaining one whose artist differs" is one of its first two.
#[derive(Default)]
struct Cycle {
    by_artist: HashMap<usize, VecDeque<(usize, usize)>>,
    heads: BTreeSet<(usize, usize)>,
}

impl Cycle {
    fn is_empty(&self) -> bool {
        self.heads.is_empty()
    }

    /// Takes the first remaining candidate whose artist is not `previous`, or else the first
    /// one; returns its pool index and artist.
    fn take(&mut self, previous: Option<usize>) -> Option<(usize, usize)> {
        let &(_, artist) = self
            .heads
            .iter()
            .find(|(_, artist)| Some(*artist) != previous)
            .or_else(|| self.heads.iter().next())?;
        let queue = self.by_artist.get_mut(&artist)?;
        let (position, index) = queue.pop_front()?;
        self.heads.remove(&(position, artist));
        if let Some(&(next, _)) = queue.front() {
            self.heads.insert((next, artist));
        }
        Some((index, artist))
    }
}

/// `1 + min(days since the last play, 60)`; a track never played weighs 61.
fn weight(last_played: Option<i64>, now_ms: i64) -> f64 {
    let days = match last_played {
        Some(at) => ((now_ms - at).max(0) / DAY_MS).min(MAX_DAYS),
        None => MAX_DAYS,
    };
    (1 + days) as f64
}

/// Numbers the pool's artists, spelled alike regardless of case: (by pool index, each number's
/// key).
fn number_artists(pool: &[Candidate]) -> (Vec<usize>, Vec<String>) {
    let mut numbers: HashMap<String, usize> = HashMap::new();
    let mut keys: Vec<String> = Vec::new();
    let artist_of = pool
        .iter()
        .map(|candidate| {
            let key = candidate.artist.to_lowercase();
            *numbers.entry(key).or_insert_with_key(|key| {
                keys.push(key.clone());
                keys.len() - 1
            })
        })
        .collect();
    (artist_of, keys)
}

impl Deck {
    pub fn new(pool: Vec<Candidate>, now_ms: i64, rng: StdRng) -> Deck {
        let (artist_of, artist_keys) = number_artists(&pool);
        Deck {
            pool,
            artist_of,
            artist_keys,
            now_ms,
            rng,
            cycle: Cycle::default(),
            last_artist: None,
        }
    }

    /// Deals from `pool` from now on, starting a fresh cycle. The artist dealt last stays
    /// spaced from the next track.
    pub fn replace_pool(&mut self, pool: Vec<Candidate>, now_ms: i64) {
        let last = self
            .last_artist
            .and_then(|artist| self.artist_keys.get(artist).cloned());
        let (artist_of, artist_keys) = number_artists(&pool);
        self.last_artist = last.and_then(|key| artist_keys.iter().position(|k| *k == key));
        self.pool = pool;
        self.artist_of = artist_of;
        self.artist_keys = artist_keys;
        self.now_ms = now_ms;
        self.cycle = Cycle::default();
    }

    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    /// The next `n` track ids (fewer only for an empty pool). Starts new cycles as needed.
    pub fn next_batch(&mut self, n: usize) -> Vec<String> {
        let mut batch = Vec::with_capacity(n);
        if self.pool.is_empty() {
            return batch;
        }
        while batch.len() < n {
            if self.cycle.is_empty() {
                self.start_cycle();
            }
            let Some((index, artist)) = self.cycle.take(self.last_artist) else {
                break;
            };
            self.last_artist = Some(artist);
            batch.push(self.pool[index].id.clone());
        }
        batch
    }

    /// Orders the whole pool by `u ^ (1 / weight)` (u uniform in (0, 1)), descending.
    fn start_cycle(&mut self) {
        let mut keyed: Vec<(f64, usize)> = self
            .pool
            .iter()
            .enumerate()
            .map(|(index, candidate)| {
                let u: f64 = self.rng.sample(Open01);
                let key = u.powf(1.0 / weight(candidate.last_played, self.now_ms));
                (key, index)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut cycle = Cycle::default();
        for (position, (_, index)) in keyed.into_iter().enumerate() {
            let artist = self.artist_of[index];
            let queue = cycle.by_artist.entry(artist).or_default();
            if queue.is_empty() {
                cycle.heads.insert((position, artist));
            }
            queue.push_back((position, index));
        }
        self.cycle = cycle;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use rand::SeedableRng;

    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(7)
    }

    fn candidate(id: String, artist: &str, last_played: Option<i64>) -> Candidate {
        Candidate {
            id,
            artist: artist.into(),
            last_played,
        }
    }

    #[test]
    fn no_repeats_until_pool_used() {
        let pool: Vec<Candidate> = (0..30)
            .map(|i| candidate(format!("t{i}"), &format!("Artist {}", i % 6), None))
            .collect();
        let all: HashSet<String> = pool.iter().map(|c| c.id.clone()).collect();
        let mut deck = Deck::new(pool, NOW, rng());
        let mut dealt = HashSet::new();
        for _ in 0..3 {
            let batch = deck.next_batch(10);
            assert_eq!(batch.len(), 10);
            dealt.extend(batch);
        }
        assert_eq!(dealt, all);
        let fourth = deck.next_batch(10);
        assert_eq!(fourth.len(), 10);
        assert!(fourth.iter().all(|id| all.contains(id)));
        assert_eq!(fourth.iter().collect::<HashSet<_>>().len(), 10);
    }

    #[test]
    fn no_adjacent_same_artist_when_avoidable() {
        let pool: Vec<Candidate> = (0..20)
            .map(|i| candidate(format!("t{i}"), if i < 10 { "A" } else { "B" }, None))
            .collect();
        let artist_of: HashMap<String, String> = pool
            .iter()
            .map(|c| (c.id.clone(), c.artist.clone()))
            .collect();
        let mut deck = Deck::new(pool, NOW, rng());
        // Two whole cycles: the boundary between them is checked too.
        let dealt = deck.next_batch(40);
        assert_eq!(dealt.len(), 40);
        for pair in dealt.windows(2) {
            assert_ne!(artist_of[&pair[0]], artist_of[&pair[1]], "{dealt:?}");
        }
    }

    #[test]
    fn single_artist_pool_still_plays() {
        let pool: Vec<Candidate> = (0..5)
            .map(|i| candidate(format!("t{i}"), "Solo", None))
            .collect();
        let mut deck = Deck::new(pool, NOW, rng());
        let batch = deck.next_batch(5);
        assert_eq!(batch.iter().collect::<HashSet<_>>().len(), 5);
    }

    #[test]
    fn least_recently_played_tend_first() {
        let mut pool: Vec<Candidate> = (0..50)
            .map(|i| candidate(format!("new{i}"), &format!("Artist {i}"), None))
            .collect();
        pool.extend(
            (0..50)
                .map(|i| candidate(format!("old{i}"), &format!("Other {i}"), Some(NOW - 60_000))),
        );
        let mut deck = Deck::new(pool, NOW, rng());
        let first = deck.next_batch(50);
        let never = first.iter().filter(|id| id.starts_with("new")).count();
        assert!(
            never >= 35,
            "only {never} of the first 50 were never played"
        );
    }

    #[test]
    fn spacing_ignores_artist_case() {
        // "Portishead" and "PORTISHEAD" are one artist: two of them never sit side by side
        // while another artist could go between.
        let pool: Vec<Candidate> = (0..20)
            .map(|i| {
                let artist = match i % 4 {
                    0 => "Portishead",
                    1 => "PORTISHEAD",
                    2 => "portishead",
                    _ => "Tricky",
                };
                candidate(format!("t{i}"), artist, None)
            })
            .collect();
        let artist_of: HashMap<String, String> = pool
            .iter()
            .map(|c| (c.id.clone(), c.artist.to_lowercase()))
            .collect();
        let mut deck = Deck::new(pool, NOW, rng());
        let dealt = deck.next_batch(20);
        // 15 Portishead and 5 Tricky: the first ten alternate, every Tricky spent between
        // two Portishead.
        for pair in dealt[..10].windows(2) {
            assert_ne!(artist_of[&pair[0]], artist_of[&pair[1]], "{dealt:?}");
        }
    }

    #[test]
    fn replaced_pool_keeps_the_spacing() {
        let pool: Vec<Candidate> = (0..4)
            .map(|i| candidate(format!("a{i}"), "A", None))
            .collect();
        let mut deck = Deck::new(pool, NOW, rng());
        assert_eq!(deck.next_batch(1).len(), 1);
        // The new pool holds the same artist in another case and one other artist: the
        // other one goes first, since "A" just played.
        let mut fresh: Vec<Candidate> = (0..4)
            .map(|i| candidate(format!("b{i}"), "a", None))
            .collect();
        fresh.push(candidate("other".into(), "B", None));
        deck.replace_pool(fresh, NOW);
        assert_eq!(deck.next_batch(1), vec!["other".to_string()]);
        let rest = deck.next_batch(4);
        assert!(rest.iter().all(|id| id.starts_with('b')), "{rest:?}");
    }

    #[test]
    fn empty_pool_deals_nothing() {
        let mut deck = Deck::new(Vec::new(), NOW, rng());
        assert!(deck.is_empty());
        assert!(deck.next_batch(3).is_empty());
    }
}
