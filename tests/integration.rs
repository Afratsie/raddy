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

//! End-to-end integration tests.
//!
//! Each test spawns the real `raddy` binary (via `CARGO_BIN_EXE_raddy`) as a
//! subprocess against a minimal in-process upstream, so the server's own tokio
//! runtime, signal handlers, and connection pools are exercised exactly as in
//! production. The M2 acceptance criteria covered here: forwarding, `to`
//! round-robin, 400/404 fallbacks, `redir`, SIGHUP reload, invalid-reload
//! retention, and the `raddy check` exit codes.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_raddy");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Reserve a free TCP port.
///
/// Ports are drawn from a per-process counter so parallel tests never collide
/// with each other, and each port is verified bindable before it is returned
/// (so a port taken by some other process is skipped).
fn free_port() -> u16 {
    static PORT_COUNTER: AtomicUsize = AtomicUsize::new(0);
    const PORT_BASE: u16 = 20_000;
    loop {
        let port = PORT_BASE + PORT_COUNTER.fetch_add(1, Ordering::Relaxed) as u16;
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
}

/// Bind a loopback listener for a test upstream, retrying with a fresh port if
/// a lingering socket (e.g. a TIME_WAIT left by a killed subprocess) blocks it.
///
/// The listener is bound in the test thread (not inside the worker) so the
/// retry loop runs before any request traffic starts.
fn bind_listener() -> (u16, TcpListener) {
    loop {
        let port = free_port();
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => return (port, listener),
            Err(_) => continue,
        }
    }
}

/// A minimal HTTP/1.1 upstream that responds `label=<name>` and counts the
/// connections it accepted.
struct EchoUpstream {
    port: u16,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl EchoUpstream {
    fn spawn(label: &str) -> (u16, EchoUpstream) {
        let (port, listener) = bind_listener();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let hits_thread = hits.clone();
        let stop_thread = stop.clone();
        let label = label.to_string();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_thread.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                hits_thread.fetch_add(1, Ordering::Relaxed);
                let label = label.clone();
                thread::spawn(move || {
                    // Read the request headers (single read suffices for a GET).
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let body = format!("label={label}");
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                });
            }
        });
        (
            port,
            EchoUpstream {
                port,
                hits,
                stop,
                handle: Some(handle),
            },
        )
    }

    fn hit_count(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }
}

impl Drop for EchoUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock the accept loop, then join.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// An upstream that keeps connections alive (no `Connection: close`) and counts
/// *distinct* connections, so a client can assert connection reuse across
/// requests and reloads.
struct KeepAliveUpstream {
    port: u16,
    connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl KeepAliveUpstream {
    fn spawn() -> (u16, KeepAliveUpstream) {
        let (port, listener) = bind_listener();
        let connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let connections_thread = connections.clone();
        let stop_thread = stop.clone();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_thread.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                connections_thread.fetch_add(1, Ordering::Relaxed);
                thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    // Serve requests on this connection until the client closes it.
                    'outer: loop {
                        head.clear();
                        loop {
                            match stream.read(&mut byte) {
                                Ok(0) | Err(_) => break 'outer,
                                Ok(_) => {
                                    head.push(byte[0]);
                                    if head.ends_with(b"\r\n\r\n") {
                                        break;
                                    }
                                }
                            }
                        }
                        let body = "label=keepalive";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        if stream.write_all(resp.as_bytes()).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (
            port,
            KeepAliveUpstream {
                port,
                connections,
                stop,
                handle: Some(handle),
            },
        )
    }

    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for KeepAliveUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A running `raddy run` subprocess plus the config file it reads.
struct RadRaddy {
    child: Child,
    port: u16,
    config_path: PathBuf,
}

impl RadRaddy {
    /// Spawn `raddy run` with a config generated by `config_for` (which receives
    /// the chosen port).
    fn spawn(config_for: impl Fn(u16) -> String) -> RadRaddy {
        Self::spawn_with_args(config_for, &[])
    }

    /// Spawn with extra CLI arguments (e.g. `--metrics-addr`, `--access-log`).
    fn spawn_with_args(config_for: impl Fn(u16) -> String, extra: &[String]) -> RadRaddy {
        loop {
            let port = free_port();
            let config = config_for(port);
            let config_path = std::env::temp_dir().join(format!(
                "raddy_integration_{}_{}.Raddyfile",
                std::process::id(),
                port
            ));
            std::fs::write(&config_path, &config).unwrap();
            let mut cmd = Command::new(BIN);
            cmd.args(["run", "-c"]).arg(&config_path);
            cmd.args(extra);
            cmd.env("RUST_LOG", "error").stderr(Stdio::inherit());
            let child = cmd.spawn().expect("failed to spawn the raddy binary");
            let mut raddy = RadRaddy {
                child,
                port,
                config_path,
            };
            if raddy.wait_for_ready() {
                return raddy;
            }
            // The child exited (bind failure) or never listened; retry.
            let _ = raddy.child.kill();
            let _ = raddy.child.wait();
        }
    }

    /// The port the server is listening on (the test should use this, not a
    /// locally captured port, since a spawn retry may have changed it).
    fn port(&self) -> u16 {
        self.port
    }

    /// Poll until the listener accepts connections, or the child exits. Returns
    /// `false` if the child failed to start or the deadline passed.
    fn wait_for_ready(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.child.try_wait().expect("try_wait failed").is_some() {
                return false;
            }
            // Require an actual HTTP response (any status), not just a TCP
            // accept: the listener can be bound before the proxy layer is ready
            // to serve, and under parallel load that window is long enough to
            // flake a test. The probe omits Host so raddy answers 400 without
            // touching any upstream (which would pollute connection counts).
            if try_request(self.port, None, "/").is_some() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Rewrite the Raddyfile and deliver a SIGHUP.
    fn reload(&mut self, config: &str) {
        std::fs::write(&self.config_path, config).unwrap();
        // SAFETY: self.child.id() is this process's own raddy child.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGHUP);
        }
    }
}

impl Drop for RadRaddy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.config_path);
    }
}

/// A parsed HTTP response.
#[derive(Debug)]
struct Response {
    status: u16,
    headers: String, // lowercased
    body: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}:");
        self.headers
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .map(str::trim)
    }
}

/// Send a request to `port`; returns `None` if the connection or read fails
/// (used for polling during reload).
fn try_request(port: u16, host: Option<&str>, path: &str) -> Option<Response> {
    try_request_hdr(port, host, path, &[])
}

/// Send a request with extra header lines (e.g. `Accept-Encoding`).
fn try_request_hdr(
    port: u16,
    host: Option<&str>,
    path: &str,
    headers: &[&str],
) -> Option<Response> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let host_line = host.map(|h| format!("Host: {h}\r\n")).unwrap_or_default();
    let extra = headers
        .iter()
        .map(|h| format!("{h}\r\n"))
        .collect::<String>();
    let request = format!("GET {path} HTTP/1.1\r\n{host_line}{extra}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    // Read exactly the head + Content-Length body (never wait for a connection
    // close that a keep-alive response may not send).
    Some(read_one_response(&mut stream))
}

fn send_request(port: u16, host: Option<&str>, path: &str) -> Response {
    try_request(port, host, path).expect("request failed")
}

/// Send a request with extra header lines (e.g. `Accept-Encoding`).
fn send_request_hdr(port: u16, host: Option<&str>, path: &str, headers: &[&str]) -> Response {
    try_request_hdr(port, host, path, headers).expect("request failed")
}

/// Read a single HTTP/1.1 response from a keep-alive connection: the head
/// (through `\r\n\r\n`) plus the body per `Content-Length`.
fn read_one_response(stream: &mut TcpStream) -> Response {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while stream.read(&mut byte).unwrap_or(0) != 0 {
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head_str = String::from_utf8_lossy(&head).to_string();
    let content_length = head_str
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).unwrap();
    }
    let status = head_str
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Response {
        status,
        headers: head_str.to_lowercase(),
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

/// Send `n` requests over a single keep-alive downstream connection.
fn keepalive_requests(port: u16, host: &str, n: usize) -> Vec<Response> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut responses = Vec::with_capacity(n);
    for _ in 0..n {
        let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        responses.push(read_one_response(&mut stream));
    }
    responses
}

/// Poll until `cond` holds or the deadline passes.
fn wait_until(cond: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut cond = cond;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for: {what}");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn forwards_to_upstream() {
    let (up_port, _up) = EchoUpstream::spawn("A");
    let raddy =
        RadRaddy::spawn(|port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n}}\n"));

    let resp = send_request(raddy.port(), Some("localhost"), "/");
    assert_eq!(resp.status, 200, "unexpected response: {resp:?}");
    assert_eq!(resp.body, "label=A");
}

#[test]
fn round_robin_across_upstreams() {
    let (a_port, a) = EchoUpstream::spawn("A");
    let (b_port, b) = EchoUpstream::spawn("B");
    let raddy = RadRaddy::spawn(|port| {
        format!(
            ":{port} {{\n    reverse_proxy {{\n        to 127.0.0.1:{a_port} 127.0.0.1:{b_port}\n    }}\n}}\n"
        )
    });

    for _ in 0..6 {
        assert_eq!(
            send_request(raddy.port(), Some("localhost"), "/").status,
            200
        );
    }
    assert!(
        a.hit_count() >= 2 && b.hit_count() >= 2,
        "expected both upstreams to serve requests: a={}, b={}",
        a.hit_count(),
        b.hit_count()
    );
}

#[test]
fn missing_host_returns_400() {
    let (up_port, _up) = EchoUpstream::spawn("A");
    let raddy =
        RadRaddy::spawn(|port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n}}\n"));

    // Wait for the server to accept requests before asserting.
    wait_until(
        || try_request(raddy.port(), None, "/").is_some(),
        "server to accept requests",
    );
    let resp = send_request(raddy.port(), None, "/");
    assert_eq!(resp.status, 400);
}

#[test]
fn unknown_host_returns_404_and_named_host_works() {
    let (up_port, _up) = EchoUpstream::spawn("A");
    let raddy = RadRaddy::spawn(|port| {
        format!("api.example.com:{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n}}\n")
    });

    let unknown = send_request(raddy.port(), Some("other.example.com"), "/");
    assert_eq!(unknown.status, 404);
    let known = send_request(raddy.port(), Some("api.example.com"), "/");
    assert_eq!(known.status, 200);
}

#[test]
fn redirect_expands_placeholders() {
    let raddy = RadRaddy::spawn(|port| {
        format!(":{port} {{\n    redir https://{{host}}{{uri}} permanent\n}}\n")
    });

    let resp = send_request(raddy.port(), Some("example.com:8080"), "/a/b?x=1");
    assert_eq!(resp.status, 308);
    assert_eq!(resp.header("location"), Some("https://example.com/a/b?x=1"));
}

#[test]
fn header_up_reaches_upstream() {
    let (up_port, _up) = EchoUpstream::spawn("A");
    let raddy = RadRaddy::spawn(|port| {
        format!(
            ":{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n    header_up X-Proxy-Port {port}\n}}\n"
        )
    });

    // The body only echoes `label=`, so this test asserts the header rewrite
    // does not break forwarding; the header value itself is asserted in the
    // `raddy check`/unit layer. (A full header-echo upstream is overkill here.)
    let resp = send_request(raddy.port(), Some("localhost"), "/");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "label=A");
}

#[test]
fn sighup_reload_swaps_config() {
    let (a_port, _a) = EchoUpstream::spawn("A");
    let (b_port, _b) = EchoUpstream::spawn("B");
    let mut raddy =
        RadRaddy::spawn(|port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{a_port}\n}}\n"));

    assert_eq!(
        send_request(raddy.port(), Some("localhost"), "/").body,
        "label=A"
    );
    raddy.reload(&format!(
        ":{} {{\n    reverse_proxy 127.0.0.1:{b_port}\n}}\n",
        raddy.port()
    ));
    wait_until(
        || try_request(raddy.port(), Some("localhost"), "/").is_some_and(|r| r.body == "label=B"),
        "reload to switch upstreams",
    );
}

#[test]
fn invalid_reload_keeps_old_config() {
    let (a_port, _a) = EchoUpstream::spawn("A");
    let mut raddy =
        RadRaddy::spawn(|port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{a_port}\n}}\n"));

    assert_eq!(
        send_request(raddy.port(), Some("localhost"), "/").body,
        "label=A"
    );
    raddy.reload("bogus config that cannot parse");
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        send_request(raddy.port(), Some("localhost"), "/").body,
        "label=A"
    );
}

#[test]
fn upstream_pool_survives_reload() {
    let (up_port, up) = KeepAliveUpstream::spawn();
    let mut raddy =
        RadRaddy::spawn(|port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n}}\n"));

    // Two requests on one downstream connection reuse a single upstream
    // connection (ADR-011 connection pooling).
    let responses = keepalive_requests(raddy.port(), "localhost", 2);
    assert!(responses.iter().all(|r| r.status == 200));
    assert_eq!(
        up.connection_count(),
        1,
        "expected the two requests to share one upstream connection"
    );

    // A reload swaps the snapshot but must not rebuild the Connector's pools.
    raddy.reload(&format!(
        ":{} {{\n    reverse_proxy 127.0.0.1:{up_port}\n    header_up X-Test yes\n}}\n",
        raddy.port()
    ));
    thread::sleep(Duration::from_millis(200));

    // A new downstream connection after the reload still reuses the pooled
    // upstream connection.
    let responses = keepalive_requests(raddy.port(), "localhost", 1);
    assert_eq!(responses[0].status, 200);
    assert_eq!(
        up.connection_count(),
        1,
        "the upstream pool must survive a reload (ADR-011)"
    );
}

#[test]
fn raddy_check_exit_codes() {
    let valid = std::env::temp_dir().join(format!(
        "raddy_check_valid_{}.Raddyfile",
        std::process::id()
    ));
    let invalid = std::env::temp_dir().join(format!(
        "raddy_check_invalid_{}.Raddyfile",
        std::process::id()
    ));
    std::fs::write(&valid, ":8080 {\n    reverse_proxy 127.0.0.1:1\n}\n").unwrap();
    std::fs::write(&invalid, "bogus\n").unwrap();

    let ok = Command::new(BIN)
        .args(["check", "-c"])
        .arg(&valid)
        .output()
        .unwrap();
    assert!(ok.status.success(), "check should exit 0 on valid config");
    let bad = Command::new(BIN)
        .args(["check", "-c"])
        .arg(&invalid)
        .output()
        .unwrap();
    assert!(
        !bad.status.success(),
        "check should exit 1 on invalid config"
    );

    let _ = std::fs::remove_file(&valid);
    let _ = std::fs::remove_file(&invalid);
}

#[test]
fn file_server_serves_static_files() {
    let dir = std::env::temp_dir().join(format!("raddy_fs_a_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("static")).unwrap();
    std::fs::write(dir.join("static/hello.txt"), "hello-static").unwrap();
    std::fs::write(dir.join("static/index.html"), "<h1>index</h1>").unwrap();
    let dir_cfg = dir.clone();
    let raddy = RadRaddy::spawn(move |port| {
        format!(
            ":{port} {{\n    handle /static/* {{\n        root {}\n        file_server\n    }}\n}}\n",
            dir_cfg.display()
        )
    });
    let port = raddy.port();

    let file = send_request(port, Some("localhost"), "/static/hello.txt");
    assert_eq!(file.status, 200);
    assert_eq!(file.body, "hello-static");

    let index = send_request(port, Some("localhost"), "/static/");
    assert_eq!(index.status, 200);
    assert!(index.body.contains("index"));

    let missing = send_request(port, Some("localhost"), "/static/nope.txt");
    assert_eq!(missing.status, 404);

    // Our client sends the raw path (no normalization), so the `..` guard
    // must reject traversal.
    let traversal = send_request(port, Some("localhost"), "/static/../../etc/passwd");
    assert_eq!(traversal.status, 404);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_server_compresses_on_accept_encoding() {
    let dir = std::env::temp_dir().join(format!("raddy_fs_b_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("hello.txt"),
        "hello compressible compressible compressible",
    )
    .unwrap();
    let dir_cfg = dir.clone();
    let raddy = RadRaddy::spawn(move |port| {
        format!(
            ":{port} {{\n    root {}\n    encode gzip\n    file_server\n}}\n",
            dir_cfg.display()
        )
    });
    let port = raddy.port();

    // raddy is up (wait_for_ready), but the very first request can transiently
    // fail under heavy parallel load; poll until it succeeds before asserting.
    wait_until(
        || try_request(port, Some("localhost"), "/hello.txt").is_some_and(|r| r.status == 200),
        "plain file_server response",
    );
    let plain = send_request(port, Some("localhost"), "/hello.txt");
    assert_eq!(plain.status, 200);
    assert_eq!(plain.body, "hello compressible compressible compressible");

    let gz = send_request_hdr(
        port,
        Some("localhost"),
        "/hello.txt",
        &["Accept-Encoding: gzip"],
    );
    assert_eq!(gz.header("content-encoding"), Some("gzip"));
    assert_ne!(
        gz.body, plain.body,
        "gzip response body must differ from the plain body"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn metrics_endpoint_reports_requests() {
    let (up_port, _up) = EchoUpstream::spawn("A");
    let metrics_port = free_port();
    let addr = format!("127.0.0.1:{metrics_port}");
    let raddy = RadRaddy::spawn_with_args(
        move |port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n}}\n"),
        &[String::from("--metrics-addr"), addr],
    );

    let resp = send_request(raddy.port(), Some("localhost"), "/");
    assert_eq!(resp.status, 200);

    // The metrics listener is a second service and the request counter is
    // recorded in the logging hook after the response, so poll until the
    // endpoint both is reachable and reports the counter.
    wait_until(
        || {
            try_request(metrics_port, None, "/metrics")
                .is_some_and(|r| r.body.contains("raddy_requests_total"))
        },
        "metrics to report the request counter",
    );
    let metrics = try_request(metrics_port, None, "/metrics").expect("metrics endpoint");
    assert!(
        metrics.body.contains("raddy_requests_total"),
        "metrics should report the request counter: {}",
        metrics.body
    );
}

#[test]
fn access_log_writes_json_lines() {
    let (up_port, _up) = EchoUpstream::spawn("A");
    let log_path = std::env::temp_dir().join(format!("raddy_access_{}.log", std::process::id()));
    let log_str = log_path.to_string_lossy().into_owned();
    let raddy = RadRaddy::spawn_with_args(
        move |port| format!(":{port} {{\n    reverse_proxy 127.0.0.1:{up_port}\n}}\n"),
        &[String::from("--access-log"), log_str],
    );

    let resp = send_request(raddy.port(), Some("localhost"), "/x");
    assert_eq!(resp.status, 200);

    wait_until(
        || {
            std::fs::read_to_string(&log_path)
                .map(|s| s.contains("\"path\":\"/x\"") && s.contains("\"status\":200"))
                .unwrap_or(false)
        },
        "access log line for the request",
    );
    let _ = std::fs::remove_file(&log_path);
}
