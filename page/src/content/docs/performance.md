---
title: Performance
description: A reproducible QPS / P99 baseline produced by a fixed load-test script.
---

> A **reproducible** QPS / P99 record produced by a fixed script on a fixed
> machine. The numbers are not targets — the reproducibility is. Re-run the same
> script on any machine to compare.

## Method and tooling

- **Tool**: the in-repo `examples/loadtest.rs`
  (`cargo run --release --example loadtest -- <url> <concurrency> <requests>`).
- **Load shape**: every request opens a **fresh TCP connection**
  (`Connection: close`) from client → raddy → upstream. This shape is dominated
  by connection-setup overhead, so it **understates** keep-alive steady-state
  throughput; it is reproducible, not a ceiling.
- **Topology**: `raddy run` (release build) proxying to a local `python3
  http.server` upstream (small `hello world` bodies), plain HTTP.
- **Metrics**: QPS (total requests / wall time), p50 / p99 (per-request round
  trip including TCP connect).

## Test environment

| Item | Value |
|---|---|
| CPU | Intel Core i5-11320H @ 3.20GHz (4 cores / 8 threads) |
| Kernel | Linux 7.1.5-arch1-2 |
| raddy | release build (`cargo build --release`) |

## Results (2026-08-05)

| Concurrency | Requests | QPS | p50 | p99 |
|---|---|---|---|---|
| 8 | 10,000 | 7,088 | 0.54ms | 2.57ms |
| 16 | 20,000 | 5,651 | 0.52ms | 3.42ms |
| 32 | 20,000 | 4,380 | 0.59ms | 3.63ms |

> QPS drops as concurrency rises — expected for the per-request-connection
> shape, where client-side connection setup becomes the bottleneck rather than
> raddy itself. p99 stays in the 2.5–3.6ms range.

## Reproduce

```bash
cargo build --release
cargo build --release --example loadtest

# Start an upstream and raddy (replace ports as needed)
python3 -m http.server 19200 --bind 127.0.0.1
./target/release/raddy run -c <Raddyfile proxying to 127.0.0.1:19200>

# Load test
./target/release/examples/loadtest http://127.0.0.1:8098/ 16 20000
```

## Future work

- Add a keep-alive load shape (connection reuse) to approach steady-state
  throughput.
- If a CI performance regression gate is added, use the **proxy overhead
  relative to bare Pingora** as the threshold, not an absolute value.
