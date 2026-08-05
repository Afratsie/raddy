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

//! A tiny HTTP load tester used to record the reproducible performance baseline.
//!
//! Usage:
//! ```text
//! cargo run --release --example loadtest -- <url> <concurrency> <requests>
//! ```
//!
//! Sends plain GET requests (a fresh connection per request, `Connection: close`)
//! and prints QPS plus p50/p99 latency. Used by `docs/PERFORMANCE.md`.

use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let url = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:8080/".into());
    let concurrency: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(16);
    let requests: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10_000);

    let (host, port, path) = parse_url(&url).expect("expected http://host:port/path");
    let per_worker = requests / concurrency;

    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..concurrency {
        let host = host.clone();
        let path = path.clone();
        handles.push(tokio::spawn(async move {
            run_worker(&host, port, &path, per_worker).await
        }));
    }
    let mut latencies = Vec::with_capacity(requests);
    for handle in handles {
        latencies.extend(handle.await.expect("worker panicked"));
    }

    let elapsed = start.elapsed();
    let qps = latencies.len() as f64 / elapsed.as_secs_f64();
    latencies.sort_by(f64::total_cmp);
    println!(
        "requests={} elapsed={:.2}s qps={:.0}",
        latencies.len(),
        elapsed.as_secs_f64(),
        qps
    );
    println!(
        "p50={:.2}ms  p99={:.2}ms",
        percentile(&latencies, 0.50) * 1000.0,
        percentile(&latencies, 0.99) * 1000.0
    );
}

/// Fire `n` sequential requests, recording each one's latency in seconds.
async fn run_worker(host: &str, port: u16, path: &str, n: usize) -> Vec<f64> {
    let mut latencies = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let mut stream = TcpStream::connect((host, port)).await.expect("connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.expect("write");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read");
        latencies.push(t0.elapsed().as_secs_f64());
    }
    latencies
}

/// Parse `http://host:port/path` into (host, port, path).
fn parse_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("http://")?;
    let (host_port, path) = match rest.split_once('/') {
        Some((hp, p)) => (hp, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (host_port.to_string(), 80),
    };
    Some((host, port, path))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
