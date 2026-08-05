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

//! Static file serving for the `file_server` directive (M5).

use crate::config::ast::Encoding;
use crate::proxy::compress;
use pingora::prelude::*;
use pingora::proxy::Session;
use std::path::{Path, PathBuf};

/// Serve the request `path` from `root`, optionally compressing the response
/// with the site's `encode` algorithms (M5).
pub async fn serve(
    session: &mut Session,
    root: &str,
    path: &str,
    encode: &[Encoding],
) -> Result<()> {
    // Only GET and HEAD are served.
    let method = session.req_header().method.clone();
    if method != http::Method::GET && method != http::Method::HEAD {
        session.respond_error(405).await?;
        return Ok(());
    }

    let Some(file_path) = resolve(root, path) else {
        session.respond_error(404).await?;
        return Ok(());
    };
    let bytes = match tokio::fs::read(&file_path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            session.respond_error(404).await?;
            return Ok(());
        }
    };

    let algo = compress::choose(
        encode,
        session
            .req_header()
            .headers
            .get(http::header::ACCEPT_ENCODING),
    );
    let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
    let body = match algo {
        Some(algo) => compress::compress(algo, &bytes),
        None => bytes,
    };

    let mut resp = ResponseHeader::build(200, None)?;
    resp.insert_header(http::header::CONTENT_TYPE, mime.as_ref())?;
    if let Some(algo) = algo {
        resp.insert_header(http::header::CONTENT_ENCODING, algo.token())?;
    }
    resp.insert_header(http::header::CONTENT_LENGTH, body.len().to_string())?;

    let end_of_stream = method == http::Method::HEAD || body.is_empty();
    session
        .write_response_header(Box::new(resp), end_of_stream)
        .await?;
    if !end_of_stream {
        session
            .write_response_body(Some(bytes::Bytes::from(body)), true)
            .await?;
    }
    Ok(())
}

/// Resolve a request path under `root`, guarding against directory traversal.
///
/// Returns `None` for paths that escape the root or do not resolve to a file
/// (a directory resolves to its `index.html`).
fn resolve(root: &str, request_path: &str) -> Option<PathBuf> {
    // Cheap first guard: reject any `..` path segment.
    if request_path.split('/').any(|seg| seg == "..") {
        return None;
    }
    let root_path = Path::new(root);
    let rel = request_path.trim_start_matches('/');
    let candidate = root_path.join(rel);
    let canonical_root = std::fs::canonicalize(root_path).ok()?;
    let canonical = std::fs::canonicalize(&candidate).ok()?;
    if !canonical.starts_with(&canonical_root) {
        return None;
    }
    if canonical.is_dir() {
        let index = canonical.join("index.html");
        let index = std::fs::canonicalize(&index).ok()?;
        return index.starts_with(&canonical_root).then_some(index);
    }
    Some(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_is_rejected() {
        assert!(resolve("/tmp", "/../../etc/passwd").is_none());
        assert!(resolve("/tmp", "/a/../b").is_none());
        assert!(resolve("/tmp", "/..").is_none());
    }

    #[test]
    fn missing_and_valid_paths() {
        // A non-existent path under an existing root resolves to None.
        let root = std::env::temp_dir();
        assert!(resolve(root.to_str().unwrap(), "/definitely_missing_raddy_file").is_none());
        // The root itself is a directory with no index.html → None.
        assert!(resolve(root.to_str().unwrap(), "/").is_none());
    }
}
