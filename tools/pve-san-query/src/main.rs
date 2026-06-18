//! pve-san-query: Query Proxmox VE hosts for SAN/FC storage information
//!
//! This tool retrieves information about running VMs on a Proxmox host,
//! their configured disks, and underlying block devices.
//! It can output the data to stdout or a file in JSON format.

use clap::Parser;
use libpve_san::get_san_storage_info;
use std::io::{self, Write};
use std::process;

/// Query Proxmox VE hosts for SAN/FC storage information
#[derive(Parser, Debug)]
#[command(name = "pve-san-query")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "Retrieve SAN/FC storage information from Proxmox VE hosts", long_about = None)]
struct Cli {
    /// The Proxmox hostname to connect to
    #[arg(long, short = 'H')]
    hostname: String,

    /// The username for authentication
    #[arg(long, short = 'u')]
    username: String,

    /// The password for authentication
    #[arg(long, short = 'P')]
    password: String,

    /// The realm for authentication (default: pam)
    #[arg(long, default_value = "pam")]
    realm: String,

    /// Use HTTP instead of HTTPS
    #[arg(long)]
    insecure: bool,

    /// Custom port (optional)
    #[arg(long)]
    port: Option<u16>,

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
        eprintln!("Connecting to: {}", cli.hostname);
        eprintln!("Username: {}", cli.username);
        eprintln!("Realm: {}", cli.realm);
        if cli.insecure {
            eprintln!("Using HTTP (insecure)");
        }
        if let Some(port) = cli.port {
            eprintln!("Port: {}", port);
        }
    }

    // Retrieve SAN storage information
    let rt = tokio::runtime::Runtime::new().map_err(|e| {
        eprintln!("Error creating runtime: {}", e);
        process::exit(1);
    });

    let result = match rt {
        Ok(ref runtime) => {
            runtime.block_on(async {
                get_san_storage_info(&cli.hostname, &cli.username, &cli.password).await
            })
        }
        Err(_) => {
            process::exit(1);
        }
    };

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
                            if let Err(e) = std::fs::write(output_path, &json) {
                                eprintln!("Error writing to file '{}': {}", output_path, e);
                                process::exit(1);
                            }
                            if cli.verbose {
                                eprintln!("Successfully wrote {} bytes to '{}'", json.len(), output_path);
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
                eprintln!("  - The Proxmox API is not accessible");
                eprintln!("  - Authentication credentials are incorrect");
                eprintln!("  - The hostname is incorrect");
                eprintln!("  - Network connectivity issues");
            }
            process::exit(1);
        }
    }
}
