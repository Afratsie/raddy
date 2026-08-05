// Copyright (c) 2026 chulingera2025
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Upstream round-robin selection (Q8).
//!
//! Selection state is mutable and lives outside the swapped snapshot (ADR-011):
//! a reload replaces the pure-data upstream lists but keeps these counters, so
//! the round-robin sequence and (via the long-lived Connector) the upstream
//! connection pools survive reloads.

use crate::config::ast::SiteKey;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A per-terminal round-robin counter.
#[derive(Debug, Default)]
struct Counter(AtomicUsize);

impl Counter {
    /// Return the next index into an upstream list of `len` entries.
    fn next(&self, len: usize) -> usize {
        self.0.fetch_add(1, Ordering::Relaxed) % len
    }
}

/// Round-robin state for every (site, terminal) pair that serves requests.
///
/// Counters are created lazily and retained across reloads (entries for
/// removed sites are simply unused).
#[derive(Debug, Default)]
pub struct UpstreamSelector {
    counters: Mutex<HashMap<(SiteKey, usize), Arc<Counter>>>,
}

impl UpstreamSelector {
    /// Create an empty selector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pick the upstream address for a site's terminal, round-robin.
    ///
    /// `terminal_index` is the position of the terminal within its site, so two
    /// terminals in the same site keep independent round-robin sequences.
    pub fn pick(
        &self,
        site_key: &SiteKey,
        terminal_index: usize,
        upstreams: &[SocketAddr],
    ) -> SocketAddr {
        debug_assert!(!upstreams.is_empty());
        let counter = self
            .counters
            .lock()
            .expect("upstream selector lock poisoned")
            .entry((site_key.clone(), terminal_index))
            .or_insert_with(|| Arc::new(Counter::default()))
            .clone();
        upstreams[counter.next(upstreams.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_cycles() {
        let sel = UpstreamSelector::new();
        let key = SiteKey::CatchAll { port: 8080 };
        let addrs = [
            SocketAddr::from(([127, 0, 0, 1], 1)),
            SocketAddr::from(([127, 0, 0, 1], 2)),
        ];
        let first = sel.pick(&key, 0, &addrs);
        let second = sel.pick(&key, 0, &addrs);
        assert_ne!(first, second);
        // Third pick wraps back to the first.
        assert_eq!(sel.pick(&key, 0, &addrs), first);
    }

    #[test]
    fn independent_terminals_have_independent_sequences() {
        let sel = UpstreamSelector::new();
        let key = SiteKey::CatchAll { port: 8080 };
        let addrs = [
            SocketAddr::from(([127, 0, 0, 1], 1)),
            SocketAddr::from(([127, 0, 0, 1], 2)),
        ];
        // Terminal 0 advances twice, terminal 1 advances twice.
        let a0 = sel.pick(&key, 0, &addrs);
        let a1 = sel.pick(&key, 0, &addrs);
        let b0 = sel.pick(&key, 1, &addrs);
        let b1 = sel.pick(&key, 1, &addrs);
        assert_ne!(a0, a1);
        assert_ne!(b0, b1);
        // Interleaving terminal 1's picks did not disturb terminal 0's sequence:
        // its third pick wraps back to a0, and terminal 1's third pick to b0.
        assert_eq!(sel.pick(&key, 0, &addrs), a0);
        assert_eq!(sel.pick(&key, 1, &addrs), b0);
    }
}
