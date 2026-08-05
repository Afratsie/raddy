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

//! CLI: the `raddy run` and `raddy check` subcommands.
//!
//! `check` and a reload share the exact same [`snapshot::build`] pipeline
//! (Q7): a config that passes `raddy check` reloads cleanly, and vice versa.

use crate::config::snapshot;
use crate::server::startup::{self, RunOptions};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "raddy",
    version,
    about = "A minimal high-performance reverse proxy gateway built on Pingora"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the reverse proxy server in the foreground.
    Run {
        /// Path to the Raddyfile.
        #[arg(short, long, default_value = "Raddyfile")]
        config: PathBuf,
        /// Directory for ACME certificates and the account credentials.
        #[arg(long, default_value = "raddy_certs")]
        cert_dir: PathBuf,
        /// ACME directory URL (Let's Encrypt production by default).
        #[arg(long, default_value = "https://acme-v02.api.letsencrypt.org/directory")]
        acme_directory: String,
        /// PEM root CA that trusts the ACME server (required for a test server
        /// such as Pebble whose CA is not publicly trusted).
        #[arg(long)]
        acme_root_pem: Option<PathBuf>,
        /// Append structured JSON access logs to this file.
        #[arg(long)]
        access_log: Option<PathBuf>,
        /// Expose Prometheus /metrics on this address (e.g. 127.0.0.1:9100).
        #[arg(long)]
        metrics_addr: Option<String>,
    },
    /// Validate a Raddyfile and exit (the same checks a reload performs).
    Check {
        /// Path to the Raddyfile.
        #[arg(short, long, default_value = "Raddyfile")]
        config: PathBuf,
    },
}

/// Entry point for the `raddy` binary.
pub fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { config } => match snapshot::build(&config) {
            Ok(_) => {
                println!("{}: ok", config.display());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        Command::Run {
            config,
            cert_dir,
            acme_directory,
            acme_root_pem,
            access_log,
            metrics_addr,
        } => {
            let opts = RunOptions {
                cert_dir,
                acme_directory,
                acme_root_pem,
                access_log,
                metrics_addr,
            };
            if let Err(e) = startup::run(&config, &opts) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }
}
