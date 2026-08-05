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

//! Startup sequence: parse → validate → snapshot → derive listeners → serve.
//!
//! The Raddyfile is fully parsed and validated before any listener is bound
//! (Q6): an invalid config returns an error and the process exits non-zero
//! without serving. The listener set is derived from the snapshot and fixed for
//! the process lifetime (ADR-010). Port 443 is served over TLS with SNI
//! dynamic certificates (M4); every other port is plain HTTP.

use crate::config::ast::{CompiledConfig, SiteKey};
use crate::config::snapshot::{self, ConfigStore};
use crate::proxy::handler::ProxyHandler;
use crate::server::acme::{AcmeManager, ChallengeStore};
use crate::server::reload;
use crate::tls::{CertStore, SniCallback};
use pingora::listeners::{tls::TlsSettings, TlsAcceptCallbacks};
use pingora::prelude::*;
use pingora::services::listening::Service;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Runtime options that come from the CLI and shape the server.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Directory for persisted certificates and the ACME account.
    pub cert_dir: PathBuf,
    /// ACME directory URL (Let's Encrypt production by default).
    pub acme_directory: String,
    /// Path to a PEM root CA that trusts the ACME server (required for Pebble).
    pub acme_root_pem: Option<PathBuf>,
    /// Append structured JSON access logs to this file (none if unset).
    pub access_log: Option<PathBuf>,
    /// Address for the Prometheus `/metrics` listener (none if unset).
    pub metrics_addr: Option<String>,
}

/// Boot the proxy server and run until a shutdown signal.
///
/// Returns an error if the Raddyfile is invalid or the server cannot be
/// constructed; the caller reports it and exits non-zero.
pub fn run(config_path: &Path, opts: &RunOptions) -> Result<(), Box<dyn Error>> {
    let snapshot = snapshot::build(config_path)?;
    let ports = snapshot.listeners();
    let email = snapshot.global.acme_email.clone();
    let startup_hosts = hosts_needing_certs(&snapshot);

    init_tracing();

    // Certificate store + ACME manager (certificates are process-lifetime and
    // survive config reloads; reload swaps only the routing snapshot).
    let cert_store = Arc::new(CertStore::new());
    let challenges = Arc::new(ChallengeStore::new());
    let acme_root_pem = match &opts.acme_root_pem {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?,
        ),
        None => None,
    };
    let acme = Arc::new(AcmeManager::new(
        cert_store.clone(),
        challenges.clone(),
        opts.acme_directory.clone(),
        acme_root_pem,
        opts.cert_dir.clone(),
        email,
    ));
    acme.load_persisted_certs();
    let issuance_tx = acme.spawn_issuance_worker();

    let config_store = Arc::new(ConfigStore::new(snapshot));
    let access_log = match &opts.access_log {
        Some(path) => Some(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|e| format!("failed to open access log {}: {e}", path.display()))?,
        )),
        None => None,
    };
    let handler = ProxyHandler::new(config_store.clone(), challenges.clone(), access_log);

    // Proactive issuance for configured named-443 hosts that lack a cached cert.
    for host in startup_hosts {
        let _ = issuance_tx.send(host);
    }

    // SNI on-demand: only issue for hosts configured as named sites (ADR-003).
    let on_miss_store = config_store.clone();
    let on_miss_tx = issuance_tx.clone();
    let on_miss: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(move |host: &str| {
        let config = on_miss_store.load();
        if is_configured_host(&config, host) {
            tracing::info!("on-demand TLS requested for authorized host {host}; queuing issuance");
            let _ = on_miss_tx.send(host.to_string());
        } else {
            tracing::warn!("on-demand TLS refused for unauthorized host {host}");
        }
    });

    let mut server = Server::new(None)?;
    server.bootstrap();

    let mut proxy = http_proxy_service(&server.configuration, handler);
    for port in ports {
        if port == 443 {
            // TLS listener with SNI dynamic certificates from the store.
            let callbacks: TlsAcceptCallbacks =
                Box::new(SniCallback::new(cert_store.clone(), on_miss.clone()));
            let settings = TlsSettings::with_callbacks(callbacks)?;
            proxy.add_tls_with_settings(&format!("0.0.0.0:{port}"), None, settings);
            tracing::info!("listening (TLS) on 0.0.0.0:{port}");
        } else {
            proxy.add_tcp(&format!("0.0.0.0:{port}"));
            tracing::info!("listening (plain HTTP) on 0.0.0.0:{port}");
        }
    }
    server.add_service(proxy);

    // Prometheus metrics listener (M5), if enabled.
    if let Some(metrics_addr) = &opts.metrics_addr {
        let mut metrics = Service::prometheus_http_service();
        metrics.add_tcp(metrics_addr);
        server.add_service(metrics);
        tracing::info!("metrics on {metrics_addr}");
    }

    // Config hot reload runs on its own thread; `run_forever` never returns.
    reload::spawn(config_path.to_path_buf(), config_store);

    server.run_forever();
}

/// Hostnames that need a certificate up front: named sites bound to 443.
fn hosts_needing_certs(config: &CompiledConfig) -> Vec<String> {
    config
        .sites
        .iter()
        .filter_map(|site| match &site.key {
            SiteKey::Named { host, port } if *port == 443 => Some(host.clone()),
            _ => None,
        })
        .collect()
}

/// Whether `host` is a named site configured on this instance.
fn is_configured_host(config: &CompiledConfig, host: &str) -> bool {
    config
        .sites
        .iter()
        .any(|site| matches!(&site.key, SiteKey::Named { host: named, .. } if named == host))
}

/// Install the global tracing subscriber (respecting `RUST_LOG`). Idempotent,
/// so `run` could be reused from a test harness.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
