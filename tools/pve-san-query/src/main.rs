//! pve-san-query: Query Proxmox VE hosts for SAN/FC storage information
//!
//! This tool retrieves information about running VMs on a Proxmox host,
//! their configured disks, and underlying block devices.
//! It can output the data to stdout or a file in JSON format.

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
                eprintln!("  - pvesh command is not available on this system");
                eprintln!("  - The Proxmox VE API is not accessible");
                eprintln!("  - The specified node does not exist");
                eprintln!("  - Network connectivity issues");
            }
            process::exit(1);
        }
    }
}
