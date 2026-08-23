//! Replay rejection: a timestamp window plus single-use nonces.
//!
//! In memory only, matching Breeze Core. A restart forgets every nonce, but
//! replaying a request still means capturing a signed one over TLS and resending
//! it inside the skew window — a negligible edge that is not worth the cost of
//! persisting, and persisting it would mean a disk write on the authentication
//! path of every request.

use std::collections::HashMap;

use crate::signing::DEFAULT_SKEW_SECONDS;

pub struct NonceCache {
    skew_seconds: u64,
    /// nonce -> the time after which it can be forgotten.
    seen: HashMap<String, f64>,
}

impl Default for NonceCache {
    fn default() -> Self {
        Self::new(DEFAULT_SKEW_SECONDS)
    }
}

impl NonceCache {
    pub fn new(skew_seconds: u64) -> Self {
        Self {
            skew_seconds,
            seen: HashMap::new(),
        }
    }

    /// The accepted drift each way. Reported to clients on a `clock_skew`
    /// rejection so they can tell how far out they are.
    pub fn skew_seconds(&self) -> u64 {
        self.skew_seconds
    }

    /// Whether a client's timestamp is close enough to ours.
    ///
    /// Unparseable is out of window, not an error: a client sending nonsense
    /// gets the same self-healing `clock_skew` response as one that is merely
    /// adrift, which is the more useful outcome for both.
    pub fn timestamp_in_window(&self, timestamp: &str, now: f64) -> bool {
        match timestamp.parse::<f64>() {
            Ok(ts) if ts.is_finite() => (now - ts).abs() <= self.skew_seconds as f64,
            _ => false,
        }
    }

    /// Accept a nonce once. `false` means it has already been used.
    pub fn check_and_store(&mut self, nonce: &str, now: f64) -> bool {
        self.sweep(now);
        if self.seen.contains_key(nonce) {
            return false;
        }
        self.seen
            .insert(nonce.to_string(), now + self.skew_seconds as f64);
        true
    }

    /// Number of nonces currently remembered. For diagnostics.
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    fn sweep(&mut self, now: f64) {
        self.seen.retain(|_, expiry| *expiry >= now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_787_488_496.0;

    #[test]
    fn a_nonce_is_accepted_once() {
        let mut c = NonceCache::new(60);
        assert!(c.check_and_store("abc", NOW));
        assert!(!c.check_and_store("abc", NOW), "second use must be refused");
        assert!(c.check_and_store("def", NOW), "a different nonce is fine");
    }

    #[test]
    fn a_nonce_is_forgotten_once_it_could_no_longer_be_replayed() {
        let mut c = NonceCache::new(60);
        assert!(c.check_and_store("abc", NOW));
        // Past the window, the timestamp check would reject a replay anyway, so
        // holding the nonce forever would only leak memory.
        assert!(c.check_and_store("abc", NOW + 61.0));
    }

    #[test]
    fn sweeping_keeps_the_cache_bounded() {
        let mut c = NonceCache::new(60);
        for i in 0..1000 {
            c.check_and_store(&format!("n{i}"), NOW);
        }
        assert_eq!(c.len(), 1000);
        // One request much later must clear the lot.
        c.check_and_store("later", NOW + 3600.0);
        assert_eq!(c.len(), 1, "expired nonces should not accumulate");
    }

    #[test]
    fn the_window_is_symmetric() {
        let c = NonceCache::new(60);
        assert!(c.timestamp_in_window("1787488496", NOW));
        assert!(c.timestamp_in_window("1787488436", NOW), "60s behind");
        assert!(c.timestamp_in_window("1787488556", NOW), "60s ahead");
        assert!(!c.timestamp_in_window("1787488435", NOW), "61s behind");
        assert!(!c.timestamp_in_window("1787488557", NOW), "61s ahead");
    }

    #[test]
    fn a_clock_ahead_of_ours_is_still_in_window() {
        // A phone whose clock runs fast is the common case, and it must not be
        // treated as an attack.
        let c = NonceCache::new(60);
        assert!(c.timestamp_in_window(&format!("{}", NOW + 30.0), NOW));
    }

    #[test]
    fn nonsense_timestamps_are_out_of_window_not_a_crash() {
        let c = NonceCache::new(60);
        for bad in ["", "abc", "NaN", "inf", "-inf", "1e400", "1787488496,0"] {
            assert!(
                !c.timestamp_in_window(bad, NOW),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn fractional_timestamps_are_accepted() {
        // Python sends time.time(), so a fractional value is normal.
        let c = NonceCache::new(60);
        assert!(c.timestamp_in_window("1787488496.123", NOW));
    }
}
