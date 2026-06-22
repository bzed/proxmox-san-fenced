//! pve-san-query: Query Proxmox VE hosts for SAN/FC storage information
//!
//! This tool retrieves information about running VMs on a Proxmox host,
//! their configured disks, and underlying block devices.
//! It can output the data to stdout or a file in JSON format.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.
//!
//! This program is distributed in the hope that it will be useful,
//! but WITHOUT ANY WARRANTY; without even the implied warranty of
//! MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
//! GNU Affero General Public License for more details.
//!
//! You should have received a copy of the GNU Affero General Public License
//! along with this program. If not, see <https://www.gnu.org/licenses/>.

use clap::Parser;
use libpve_san::get_san_storage_info_sync;
use std::io::{self, Write};
use std::process;

/// Query Proxmox VE hosts for SAN/FC storage information
#[derive(Parser, Debug)]
#[command(name = "pve-san-query")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "Retrieve SAN/FC storage information from Proxmox VE hosts", long_about = None)]
struct Cli {
    /// The Proxmox node name to query
    #[arg(long, short = 'n')]
    node: String,

    /// The output file (default: stdout)
    #[arg(long, short = 'o')]
    output: Option<String>,

    /// Pretty print JSON output
    #[arg(long, short = 'p')]
    pretty: bool,

    /// Verbose output
    #[arg(long, short = 'v')]
    verbose: bool,
}

fn main() {
    let cli = Cli::parse();

    if cli.verbose {
        eprintln!("Querying node: {}", cli.node);
    }

    // Retrieve SAN storage information
    let result = get_san_storage_info_sync(&cli.node);

    match result {
        Ok(data) => {
            let json_output = if cli.pretty {
                serde_json::to_string_pretty(&data)
            } else {
                serde_json::to_string(&data)
            };

            match json_output {
                Ok(json) => {
                    match &cli.output {
                        Some(output_path) => {
                            let validated_path = match validate_output_path(output_path) {
                                Ok(p) => p,
                                Err(err) => {
                                    eprintln!("Invalid output path: {err}");
                                    process::exit(1);
                                }
                            };
                            if let Err(e) = std::fs::write(&validated_path, &json) {
                                eprintln!("Error writing to file '{}': {}", output_path, e);
                                process::exit(1);
                            }
                            if cli.verbose {
                                eprintln!(
                                    "Successfully wrote {} bytes to '{}'",
                                    json.len(),
                                    output_path
                                );
                            }
                        }
                        None => {
                            // Output to stdout
                            if let Err(e) = io::stdout().write_all(json.as_bytes()) {
                                eprintln!("Error writing to stdout: {}", e);
                                process::exit(1);
                            }
                            if cli.verbose {
                                eprintln!("Successfully wrote {} bytes to stdout", json.len());
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error serializing to JSON: {}", e);
                    process::exit(1);
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            if cli.verbose {
                eprintln!("This error typically occurs when:");
                eprintln!("  - pvesh command is not available on this system");
                eprintln!("  - The Proxmox VE API is not accessible");
                eprintln!("  - The specified node does not exist");
                eprintln!("  - Network connectivity issues");
            }
            process::exit(1);
        }
    }
}

fn validate_output_path(path: &str) -> Result<std::path::PathBuf, String> {
    let p = std::path::Path::new(path);
    if p == std::path::Path::new("/dev/null") {
        return Ok(p.to_path_buf());
    }
    let abs_path = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {e}"))?
            .join(p)
    };

    for component in abs_path.components() {
        if let std::path::Component::ParentDir = component {
            return Err("Path traversal (..) is not allowed in output path".to_string());
        }
    }

    let cwd = std::env::current_dir().unwrap_or_default();
    let is_in_tmp = abs_path.starts_with("/tmp") || abs_path.starts_with("/var/tmp");
    let is_in_cwd = abs_path.starts_with(&cwd);

    if !is_in_tmp && !is_in_cwd {
        return Err("Output path must be located within /tmp, /var/tmp, or the current working directory".to_string());
    }

    Ok(abs_path)
}
