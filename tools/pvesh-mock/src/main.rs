//! Mock pvesh command for testing
//!
//! This tool simulates the pvesh CLI command by parsing arguments and
//! returning appropriate test data from JSON files.
//!
//! Usage:
//!   pvesh-mock ls /nodes/pve001/qemu --output-format json
//!   pvesh-mock get /nodes/pve001/qemu/104/config --output-format json
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
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

/// Default test data directory relative to the binary
const DEFAULT_TEST_DATA_DIR: &str = "test-data/pvesh";

/// Mock pvesh command
#[derive(Parser, Debug)]
#[command(name = "pvesh-mock")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "Mock pvesh command for testing", long_about = None)]
struct Cli {
    /// The command to execute (ls, get)
    #[arg(value_enum)]
    command: CommandType,

    /// The path to query (e.g., /nodes/pve001/qemu)
    path: String,

    /// Output format
    #[arg(long, short = 'o', value_name = "FORMAT")]
    output_format: Option<String>,

    /// Verbose output
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
#[allow(clippy::upper_case_acronyms)]
enum CommandType {
    #[value(alias = "ls")]
   Ls,
    #[value(alias = "get")]
    Get,
}

fn main() {
    let cli = Cli::parse();

    if cli.verbose {
        eprintln!("pvesh-mock: command={:?}, path={}, output_format={:?}", 
                 cli.command, cli.path, cli.output_format);
    }

    // Parse the path to extract node and vmid if present
    let path_parts: Vec<&str> = cli.path.trim_matches('/').split('/').collect();
    
    if cli.verbose {
        eprintln!("Path parts: {:?}", path_parts);
    }

    // Determine test data directory
    // Try PVE_SAN_TEST_DATA_DIR env var first
    let test_data_dir = if let Ok(dir) = env::var("PVE_SAN_TEST_DATA_DIR") {
        PathBuf::from(dir)
    } else {
        // Try relative to the binary location
        // When built in the workspace, the binary is at target/debug/pvesh-mock
        // and the test data is at test-data/pvesh
        // So we need to go up from the binary to find the test data
        let mut path = env::current_exe().unwrap();
        // Go up 3 levels: target/debug/pvesh-mock -> target/debug -> target -> workspace root
        for _ in 0..3 {
            path = path.parent().unwrap().to_path_buf();
        }
        path.join(DEFAULT_TEST_DATA_DIR)
    };

    if cli.verbose {
        eprintln!("Test data directory: {}", test_data_dir.display());
    }

    let response = match (cli.command, &path_parts[..]) {
        (CommandType::Ls, ["nodes", node, "qemu"]) => {
            // List VMs for a node
            handle_ls_nodes_qemu(node, &test_data_dir)
        }
        (CommandType::Get, ["nodes", node, "qemu", vmid, "config"]) => {
            // Get VM config
            handle_get_vm_config(node, vmid, &test_data_dir)
        }
        _ => {
            eprintln!("Error: Unsupported path for command: {:?}/{:?} ({:?})", 
                     cli.command, cli.path, path_parts);
            process::exit(1);
        }
    };

    let output_format = cli.output_format.as_deref().unwrap_or("json");
    
    match output_format {
        "json" | "json-pretty" => {
            // Output as JSON
            let json = match response {
                Some(json) => json,
                None => {
                    eprintln!("Error: No test data found for {:?}/{}", cli.command, cli.path);
                    process::exit(1);
                }
            };
            
            if output_format == "json-pretty" {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json) {
                    let pretty = serde_json::to_string_pretty(&parsed).unwrap();
                    io::stdout().write_all(pretty.as_bytes()).unwrap();
                } else {
                    // Not valid JSON, just output as-is
                    io::stdout().write_all(json.as_bytes()).unwrap();
                }
            } else {
                io::stdout().write_all(json.as_bytes()).unwrap();
            }
        }
        _ => {
            // For non-JSON formats, we don't support them in mock
            eprintln!("Error: Unsupported output format: {}", output_format);
            process::exit(1);
        }
    }
}

fn handle_ls_nodes_qemu(node: &str, test_data_dir: &PathBuf) -> Option<String> {
    // Look for test data file
    let filename = format!("get_nodes/{}_qemu.json", node);
    let filepath = test_data_dir.join(&filename);
    
    if let Ok(data) = fs::read_to_string(&filepath) {
        return Some(data);
    }
    
    // Try with default node name
    let default_filepath = test_data_dir.join("get_nodes/pve001_qemu.json");
    if let Ok(data) = fs::read_to_string(&default_filepath) {
        return Some(data);
    }
    
    None
}

fn handle_get_vm_config(_node: &str, vmid: &str, test_data_dir: &PathBuf) -> Option<String> {
    // Look for test data file in config subdirectory
    let filename = format!("{}.json", vmid);
    let filepath = test_data_dir
        .join("get_nodes")
        .join("config")
        .join(&filename);
    
    if let Ok(data) = fs::read_to_string(&filepath) {
        return Some(data);
    }
    
    None
}
