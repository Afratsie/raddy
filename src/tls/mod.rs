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

//! TLS certificate store and the SNI dynamic-certificate callback (M4).
//!
//! [`CertStore`] holds issued certificates in memory, keyed by hostname, and is
//! process-lifetime (across config reloads — certificates are not part of the
//! swapped snapshot). [`SniCallback`] implements pingora's `TlsAccept` so the
//! TLS handshake for a hostname is answered from the store; a miss triggers the
//! on-demand issuance path (authorized by the `ask` callback per ADR-003).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use pingora::listeners::TlsAccept;
use pingora::protocols::tls::TlsRef;
use pingora::tls::{ext, pkey::PKey, ssl::NameType, x509::X509};
use pingora::utils::tls::CertKey;

/// Process-lifetime store of certificates keyed by hostname.
#[derive(Debug, Default)]
pub struct CertStore {
    certs: RwLock<HashMap<String, Arc<CertKey>>>,
}

impl CertStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the certificate for a hostname.
    pub fn get(&self, host: &str) -> Option<Arc<CertKey>> {
        self.certs
            .read()
            .expect("cert store lock poisoned")
            .get(host)
            .cloned()
    }

    /// Whether a hostname has a certificate.
    pub fn has(&self, host: &str) -> bool {
        self.certs
            .read()
            .expect("cert store lock poisoned")
            .contains_key(host)
    }

    /// Insert or replace the certificate for a hostname.
    pub fn store(&self, host: &str, cert: CertKey) {
        self.certs
            .write()
            .expect("cert store lock poisoned")
            .insert(host.to_string(), Arc::new(cert));
    }
}

/// The SNI callback that answers a TLS handshake from the certificate store.
///
/// When the requested hostname has no certificate, `on_miss` is invoked (the
/// ACME on-demand path) and the handshake proceeds without a certificate, so it
/// fails; the client is expected to retry once issuance completes.
pub struct SniCallback {
    store: Arc<CertStore>,
    on_miss: Arc<dyn Fn(&str) + Send + Sync>,
}

impl SniCallback {
    /// Create a callback backed by `store`; `on_miss` fires for unknown SNI.
    pub fn new(store: Arc<CertStore>, on_miss: Arc<dyn Fn(&str) + Send + Sync>) -> Self {
        Self { store, on_miss }
    }
}

#[async_trait]
impl TlsAccept for SniCallback {
    async fn certificate_callback(&self, ssl: &mut TlsRef) {
        // Copy the SNI out (lowercased, as config hosts are normalized) so the
        // immutable borrow of `ssl` ends before we mutate it via `ext::ssl_use_*`.
        let Some(sni) = ssl
            .servername(NameType::HOST_NAME)
            .map(str::to_owned)
            .map(|s| s.to_ascii_lowercase())
        else {
            // No SNI; there is nothing to route on, so leave the handshake
            // without a certificate.
            return;
        };
        match self.store.get(&sni) {
            Some(cert) => {
                if let Err(e) = ext::ssl_use_certificate(ssl, cert.leaf()) {
                    tracing::warn!("failed to set certificate for {sni}: {e}");
                    return;
                }
                for intermediate in cert.intermediates() {
                    let _ = ext::ssl_add_chain_cert(ssl, intermediate);
                }
                if let Err(e) = ext::ssl_use_private_key(ssl, cert.key()) {
                    tracing::warn!("failed to set private key for {sni}: {e}");
                }
            }
            None => {
                tracing::warn!("no certificate for SNI '{sni}'; triggering on-demand issuance");
                (self.on_miss)(&sni);
            }
        }
    }
}

/// Parse a PEM certificate chain and a PEM private key into a [`CertKey`].
pub fn cert_key_from_pem(cert_chain_pem: &str, key_pem: &str) -> Result<CertKey, String> {
    let certs = X509::stack_from_pem(cert_chain_pem.as_bytes())
        .map_err(|e| format!("failed to parse certificate chain: {e}"))?;
    if certs.is_empty() {
        return Err("certificate chain is empty".to_string());
    }
    let key = PKey::private_key_from_pem(key_pem.as_bytes())
        .map_err(|e| format!("failed to parse private key: {e}"))?;
    Ok(CertKey::new(certs, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cert_store_get_store_has() {
        // Build a trivial CertKey from a generated self-signed cert.
        let (cert_pem, key_pem) = rcgen_test_cert("example.com");
        let cert = cert_key_from_pem(&cert_pem, &key_pem).unwrap();
        let store = CertStore::new();
        assert!(!store.has("example.com"));
        store.store("example.com", cert);
        assert!(store.has("example.com"));
        assert!(store.get("example.com").is_some());
        assert!(store.get("other.com").is_none());
    }

    #[test]
    fn parses_pem_roundtrip() {
        let (cert_pem, key_pem) = rcgen_test_cert("api.test");
        let cert = cert_key_from_pem(&cert_pem, &key_pem).unwrap();
        // A self-signed cert parses as a leaf with no intermediates, and the
        // leaf re-serializes to valid PEM.
        assert!(cert.intermediates().is_empty());
        let leaf_pem = String::from_utf8(cert.leaf().to_pem().unwrap()).unwrap();
        assert!(leaf_pem.starts_with("-----BEGIN CERTIFICATE-----"));
    }

    /// Generate a self-signed certificate via `rcgen` (a dev/test-only helper;
    /// production certificates come from ACME).
    fn rcgen_test_cert(host: &str) -> (String, String) {
        let cert = rcgen::generate_simple_self_signed(vec![host.to_string()])
            .expect("failed to generate test certificate");
        (cert.cert.pem(), cert.signing_key.serialize_pem())
    }

    #[tokio::test]
    async fn sni_serves_stored_certificate() {
        use pingora::listeners::TlsAcceptCallbacks;
        use pingora::protocols::tls::server::handshake_with_callback;
        use pingora::protocols::tls::SslStream;
        use pingora::tls::ssl::{self, SslAcceptor, SslMethod};
        use std::pin::Pin;

        // Store a self-signed cert for a host, then verify a TLS handshake with
        // that SNI serves exactly it.
        let store = Arc::new(CertStore::new());
        let (cert_pem, key_pem) = rcgen_test_cert("example.test");
        store.store(
            "example.test",
            cert_key_from_pem(&cert_pem, &key_pem).unwrap(),
        );

        let callbacks: TlsAcceptCallbacks = Box::new(SniCallback::new(store, Arc::new(|_| {})));
        let acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls())
            .unwrap()
            .build();
        let (client, server) = tokio::io::duplex(8192);

        // Server handshake runs concurrently; it must stay alive (not see a
        // closed pipe) until the client stream is dropped at the end of the test.
        let server_task =
            tokio::spawn(
                async move { handshake_with_callback(&acceptor, server, &callbacks).await },
            );

        // Client connects with SNI and reads back the served leaf certificate.
        let ssl_context = ssl::SslContext::builder(SslMethod::tls()).unwrap().build();
        let mut ssl = ssl::Ssl::new(&ssl_context).unwrap();
        ssl.set_hostname("example.test").unwrap();
        ssl.set_verify(ssl::SslVerifyMode::NONE);
        let mut client_stream = SslStream::new(ssl, client).unwrap();
        Pin::new(&mut client_stream).connect().await.unwrap();
        let served_pem = String::from_utf8(
            client_stream
                .ssl()
                .peer_certificate()
                .unwrap()
                .to_pem()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(served_pem.trim(), cert_pem.trim());

        server_task
            .await
            .unwrap()
            .expect("server handshake should succeed");
    }
}
