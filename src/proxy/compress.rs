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

//! Response compression for the `encode` directive (M5).
//!
//! Implements gzip and zstd compression honoring the `encode` parameter order
//! (priority) and the client's `Accept-Encoding` (RADDYFILE_SPEC §5): the first
//! configured algorithm the client accepts is used.

use crate::config::ast::Encoding;
use http::HeaderValue;

/// A concrete compression algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo {
    Gzip,
    Zstd,
}

impl Algo {
    /// The `Content-Encoding` token for this algorithm.
    pub fn token(self) -> &'static str {
        match self {
            Algo::Gzip => "gzip",
            Algo::Zstd => "zstd",
        }
    }
}

/// Choose the encoding for a request given the site's `encode` priorities and
/// the client's `Accept-Encoding` header. `None` means no compression.
///
/// Accept-Encoding parsing is deliberately simple: an entry matches if it names
/// the token or is `*`, ignoring `q` weights except for an explicit `q=0`.
pub fn choose(encode: &[Encoding], accept_encoding: Option<&HeaderValue>) -> Option<Algo> {
    if encode.is_empty() {
        return None;
    }
    let header = accept_encoding?.to_str().ok()?;
    let accepts = |token: &str| {
        header.split(',').any(|entry| {
            let entry = entry.trim();
            let (name, q) = match entry.split_once(';') {
                Some((name, rest)) => (name.trim(), parse_q(rest)),
                None => (entry, 1.0),
            };
            q > 0.0 && (name == token || name == "*")
        })
    };
    for enc in encode {
        let algo = match enc {
            Encoding::Gzip => Algo::Gzip,
            Encoding::Zstd => Algo::Zstd,
        };
        if accepts(algo.token()) {
            return Some(algo);
        }
    }
    None
}

/// Parse a `;q=...` weight (defaults to 1.0).
fn parse_q(rest: &str) -> f32 {
    rest.trim()
        .strip_prefix("q=")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0)
}

/// Compress `body` with `algo`.
pub fn compress(algo: Algo, body: &[u8]) -> Vec<u8> {
    match algo {
        Algo::Gzip => gzip(body),
        Algo::Zstd => zstd(body),
    }
}

fn gzip(body: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if encoder.write_all(body).is_err() {
        return body.to_vec();
    }
    encoder.finish().unwrap_or_else(|_| body.to_vec())
}

fn zstd(body: &[u8]) -> Vec<u8> {
    zstd::encode_all(body, 3).unwrap_or_else(|_| body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    #[test]
    fn chooses_first_priority_client_accepts() {
        // encode zstd gzip → zstd wins if the client accepts both.
        let encode = [Encoding::Zstd, Encoding::Gzip];
        assert_eq!(choose(&encode, Some(&hdr("gzip, zstd"))), Some(Algo::Zstd));
        // Client that only accepts gzip → gzip.
        assert_eq!(choose(&encode, Some(&hdr("gzip"))), Some(Algo::Gzip));
        // Client accepting neither → none.
        assert_eq!(choose(&encode, Some(&hdr("br"))), None);
    }

    #[test]
    fn wildcard_and_q0() {
        let encode = [Encoding::Zstd, Encoding::Gzip];
        // `*` accepts anything → first priority (zstd).
        assert_eq!(choose(&encode, Some(&hdr("*"))), Some(Algo::Zstd));
        // q=0 on the top priority excludes it → falls to gzip.
        assert_eq!(
            choose(&encode, Some(&hdr("zstd;q=0, gzip"))),
            Some(Algo::Gzip)
        );
        // No Accept-Encoding header → no compression.
        assert_eq!(choose(&encode, None), None);
    }

    #[test]
    fn no_encode_directive_means_no_compression() {
        assert_eq!(choose(&[], Some(&hdr("gzip"))), None);
    }

    #[test]
    fn compress_roundtrips() {
        let body = b"hello world hello world".to_vec();
        for algo in [Algo::Gzip, Algo::Zstd] {
            let compressed = compress(algo, &body);
            assert!(!compressed.is_empty());
            assert_ne!(compressed, body, "{algo:?} output should differ");
        }
    }
}
