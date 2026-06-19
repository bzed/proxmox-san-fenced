//! mpath-query: Query multipathd via its abstract namespace socket
//!
//! This tool sends commands to the multipathd daemon and retrieves the response.
//! By default, it sends "show maps json" and outputs to stdout or a specified file.
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

use clap::{Parser, Subcommand};
use std::io::{self, Write};
use std::process;

use libmultipath::{send_multipath_command_to_socket, DEFAULT_SOCKET};

/// Query multipathd daemon via its abstract namespace socket
#[derive(Parser, Debug)]
#[command(name = "mpath-query")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "Send commands to multipathd and retrieve responses", long_about = None)]
struct Cli {
    /// The command to send to multipathd (default: "show maps json")
    #[arg(long, short, default_value = "show maps json")]
    command: String,

    /// The output file (default: stdout)
    #[arg(long, short)]
    output: Option<String>,

    /// The socket path to connect to (default: @/org/kernel/linux/storage/multipathd)
    /// Use --socket @/org/kernel/linux/storage/multipathd-mock for testing with mpath-mockd
    #[arg(long, default_value = DEFAULT_SOCKET)]
    socket: String,

    /// Verbose output
    #[arg(long, short)]
    verbose: bool,

    /// Subcommands (alternative to direct command argument)
    #[command(subcommand)]
    subcommand: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show multipath maps in JSON format
    #[command(alias = "maps")]
    ShowMapsJson,
    /// Show multipath topology
    #[command(alias = "topology")]
    ShowTopology,
    /// Show multipath configuration
    #[command(alias = "config")]
    ShowConfig,
    /// Show multipath status
    #[command(alias = "status")]
    ShowStatus,
    /// List multipaths
    #[command(alias = "list")]
    ListMaps,
}

fn main() {
    let cli = Cli::parse();

    // Determine the command to send
    let command = match &cli.subcommand {
        Some(Commands::ShowMapsJson) => "show maps json",
        Some(Commands::ShowTopology) => "show topology",
        Some(Commands::ShowConfig) => "show config",
        Some(Commands::ShowStatus) => "show status",
        Some(Commands::ListMaps) => "list maps",
        None => &cli.command,
    };

    if cli.verbose {
        eprintln!("Connecting to socket: {}", cli.socket);
        eprintln!("Sending command: {}", command);
    }

    match send_multipath_command_to_socket(&cli.socket, command) {
        Ok(data) => {
            match &cli.output {
                Some(output_path) => {
                    if let Err(e) = std::fs::write(output_path, &data) {
                        eprintln!("Error writing to file '{}': {}", output_path, e);
                        process::exit(1);
                    }
                    if cli.verbose {
                        eprintln!("Successfully wrote {} bytes to '{}'", data.len(), output_path);
                    }
                }
                None => {
                    // Output to stdout
                    if let Err(e) = io::stdout().write_all(data.as_bytes()) {
                        eprintln!("Error writing to stdout: {}", e);
                        process::exit(1);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            if cli.verbose {
                eprintln!("This error typically occurs when:");
                eprintln!("  - multipathd is not running (check with: systemctl status multipathd)");
                eprintln!("  - The socket path is incorrect");
                eprintln!("  - Connection timed out");
            }
            process::exit(1);
        }
    }
}
