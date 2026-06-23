//! Mock multipathd daemon for testing
//!
//! This daemon listens on an abstract namespace socket and responds to
//! multipath commands with test data from JSON files.
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
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use std::io::{Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};

/// Default socket path for the mock daemon
/// Note: Use a different socket than the real multipathd to avoid conflicts
const DEFAULT_SOCKET: &str = "@/org/kernel/linux/storage/multipathd-mock";

/// Default file for "show maps json" command
const DEFAULT_SHOW_MAPS_JSON_FILE: &str = "all_active_running.json";

/// Mock multipathd daemon
#[derive(Parser, Debug)]
#[command(name = "mpath-mockd")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "Mock multipathd daemon for testing", long_about = None)]
struct Cli {
    /// The socket path to listen on (default: @/org/kernel/linux/storage/multipathd)
    #[arg(long, default_value = DEFAULT_SOCKET)]
    socket: String,

    /// The directory containing test data JSON files
    #[arg(long, default_value = "test-data/multipathd")]
    test_data_dir: PathBuf,

    /// Verbose output
    #[arg(long, short)]
    verbose: bool,

    /// File mappings for commands (format: command=filename or command=file1,file2,file3)
    /// Can be specified multiple times. Files are cycled through in round-robin fashion.
    /// Example: --file-map "show maps json=all_active_running.json,failed_all_timeout.json"
    #[arg(long, value_name = "command=file[s]", action = clap::ArgAction::Append)]
    file_map: Vec<String>,
}

/// Tracks the current index for cycling through files for each command
#[derive(Debug)]
struct FileCounters {
    indices: Mutex<HashMap<String, usize>>,
}

impl FileCounters {
    fn new() -> Self {
        FileCounters {
            indices: Mutex::new(HashMap::new()),
        }
    }

    /// Get the next index for a command and increment it (wraps around)
    fn next_index(&self, command: &str, max: usize) -> usize {
        let mut indices = self.indices.lock().unwrap();
        let current = indices.entry(command.to_string()).or_insert(0);
        let result = *current;
        *current = (*current + 1) % max;
        result
    }
}

/// Maps command names to their default subdirectories
fn command_to_subdir(command: &str) -> &str {
    match command {
        "show maps json" => "show_maps_json",
        "show topology" => "show_topology",
        "list maps" => "list_maps",
        "show status" => "show_status",
        "show config" => "show_config",
        _ => "",
    }
}

/// Returns the default filename for a command
fn default_filename_for_command(command: &str) -> &str {
    match command {
        "show maps json" => DEFAULT_SHOW_MAPS_JSON_FILE,
        "show topology" => "show_topology.txt",
        "list maps" => "list_maps.txt",
        "show status" => "show_status.txt",
        "show config" => "show_config.txt",
        _ => "",
    }
}

fn main() {
    let cli = Cli::parse();

    // Parse custom file mappings from CLI
    // Format: command=file1,file2,file3 or command=single_file
    let mut custom_mappings: HashMap<String, Vec<String>> = HashMap::new();
    for mapping in &cli.file_map {
        if let Some((command, files)) = mapping.split_once('=') {
            let command = command.trim().to_string();
            let file_list: Vec<String> = files.split(',').map(|s| s.trim().to_string()).collect();
            custom_mappings.insert(command, file_list);
        } else {
            eprintln!(
                "Warning: Invalid file mapping format '{}', expected command=file[s]",
                mapping
            );
        }
    }

    // Load test data files - now stores Vec<String> of file contents for cycling
    // List of known commands
    let known_commands = [
        "show maps json",
        "show topology",
        "list maps",
        "show status",
        "show config",
    ];

    let mut command_responses: HashMap<String, Vec<String>> = HashMap::new();

    for &command in &known_commands {
        // Check if there's a custom mapping for this command
        if let Some(file_list) = custom_mappings.get(command) {
            let mut file_contents = Vec::new();
            for filename in file_list {
                let filepath = cli.test_data_dir.join(filename);
                if let Ok(data) = fs::read_to_string(&filepath) {
                    file_contents.push(data);
                    if cli.verbose {
                        eprintln!(
                            "Loaded test data for '{}' from {} (custom mapping)",
                            command,
                            filepath.display()
                        );
                    }
                } else {
                    eprintln!(
                        "Warning: Could not load custom test data from {}",
                        filepath.display()
                    );
                }
            }
            if !file_contents.is_empty() {
                command_responses.insert(command.to_string(), file_contents);
            }
            continue;
        }

        // Use default subdirectory-based lookup
        let subdir = command_to_subdir(command);
        if !subdir.is_empty() {
            // Try to load all files from the subdirectory
            let mut file_contents = Vec::new();
            let default_file = default_filename_for_command(command);

            // First, try the default file
            let default_filepath = cli.test_data_dir.join(subdir).join(default_file);
            if let Ok(data) = fs::read_to_string(&default_filepath) {
                file_contents.push(data);
                if cli.verbose {
                    eprintln!(
                        "Loaded test data for '{}' from {}",
                        command,
                        default_filepath.display()
                    );
                }
            }

            // Then try to load all other files in the subdirectory
            if let Ok(dir_entries) = fs::read_dir(cli.test_data_dir.join(subdir)) {
                let mut sorted_entries: Vec<_> = dir_entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
                    .collect();
                sorted_entries.sort_by_key(|e| e.file_name());

                for entry in sorted_entries {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    // Skip the default file since we already loaded it
                    if filename != default_file {
                        let filepath = cli.test_data_dir.join(subdir).join(&filename);
                        if let Ok(data) = fs::read_to_string(&filepath) {
                            file_contents.push(data);
                            if cli.verbose {
                                eprintln!(
                                    "Loaded additional test data for '{}' from {}",
                                    command,
                                    filepath.display()
                                );
                            }
                        }
                    }
                }
            }

            if !file_contents.is_empty() {
                command_responses.insert(command.to_string(), file_contents);
            } else {
                // Try without subdirectory (for backwards compatibility)
                let flat_filepath = cli.test_data_dir.join(default_file);
                if let Ok(data) = fs::read_to_string(&flat_filepath) {
                    command_responses.insert(command.to_string(), vec![data]);
                    if cli.verbose {
                        eprintln!(
                            "Loaded test data for '{}' from {}",
                            command,
                            flat_filepath.display()
                        );
                    }
                } else {
                    eprintln!("Warning: Could not load test data for '{}'", command);
                }
            }
        }
    }

    // Add a default response for unknown commands
    command_responses
        .entry("show maps json".to_string())
        .or_insert_with(|| {
            vec![r#"{"major_version": 0, "minor_version": 1, "maps": []}"#.to_string()]
        });

    let command_responses = Arc::new(RwLock::new(command_responses));
    let file_counters = Arc::new(FileCounters::new());

    // Create socket listener
    let listener = match create_abstract_listener(&cli.socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Error creating socket: {e}");
            std::process::exit(1);
        }
    };

    if cli.verbose {
        eprintln!("Listening on abstract namespace socket: {}", cli.socket);
        eprintln!("PID: {}", std::process::id());
    }

    // Accept connections in a loop
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error accepting connection: {e}");
                continue;
            }
        };

        if cli.verbose {
            eprintln!("Accepted connection");
        }

        // Spawn a thread to handle the connection
        let command_responses_clone = command_responses.clone();
        let file_counters_clone = file_counters.clone();
        thread::spawn(move || {
            handle_connection(
                stream,
                command_responses_clone,
                file_counters_clone,
                cli.verbose,
            );
        });
    }
}

/// Creates an abstract namespace socket listener
fn create_abstract_listener(socket_path: &str) -> io::Result<UnixListener> {
    let normalized = socket_path.strip_prefix('@').unwrap_or(socket_path);
    if normalized == "org/kernel/linux/storage/multipathd"
        || normalized == "/org/kernel/linux/storage/multipathd"
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Cannot bind to the real system multipathd socket path",
        ));
    }

    let addr = SocketAddr::from_abstract_name(normalized.as_bytes())?;
    UnixListener::bind_addr(&addr)
}

/// Handles a single client connection
fn handle_connection(
    mut stream: UnixStream,
    command_responses: Arc<RwLock<HashMap<String, Vec<String>>>>,
    file_counters: Arc<FileCounters>,
    verbose: bool,
) {
    // Read command length (8 bytes, little-endian)
    let mut len_bytes = [0u8; 8];
    if let Err(e) = stream.read_exact(&mut len_bytes) {
        if verbose {
            eprintln!("Error reading command length: {e}");
        }
        return;
    }

    let cmd_len = u64::from_le_bytes(len_bytes) as usize;
    if verbose {
        eprintln!("Command length: {cmd_len}");
    }

    // Validate command length to prevent OOM attacks
    const MAX_CMD_LEN: usize = 1024 * 1024; // 1 MB max command length
    if cmd_len > MAX_CMD_LEN {
        if verbose {
            eprintln!("Command length {cmd_len} exceeds maximum of {MAX_CMD_LEN}");
        }
        return;
    }

    // Read command
    let mut command_buf = vec![0u8; cmd_len];
    if let Err(e) = stream.read_exact(&mut command_buf) {
        if verbose {
            eprintln!("Error reading command: {e}");
        }
        return;
    }

    // Convert to string (strip null terminator if present)
    let command = if let Some(pos) = command_buf.iter().position(|&b| b == 0) {
        String::from_utf8_lossy(&command_buf[..pos]).into_owned()
    } else {
        String::from_utf8_lossy(&command_buf).into_owned()
    };

    if verbose {
        eprintln!("Received command: '{command}'");
    }

    // Look up response - now uses cycling through available files
    let response = {
        let command_responses_read = command_responses.read().unwrap();
        if let Some(file_list) = command_responses_read.get(&command) {
            if file_list.len() == 1 {
                // Single file, no cycling needed
                file_list[0].clone()
            } else {
                // Multiple files, cycle through them
                let index = file_counters.next_index(&command, file_list.len());
                if verbose {
                    eprintln!("Using file index {index} for command '{command}'");
                }
                file_list[index].clone()
            }
        } else if let Some(file_list) = command_responses_read.get("show maps json") {
            // Fallback to show maps json responses
            if file_list.len() == 1 {
                file_list[0].clone()
            } else {
                let index = file_counters.next_index("show maps json", file_list.len());
                file_list[index].clone()
            }
        } else {
            if verbose {
                eprintln!("No response for command '{command}', using empty");
            }
            r#"{"error": "unknown command"}"#.to_string()
        }
    };

    // Send response length (8 bytes, little-endian)
    let resp_bytes = response.as_bytes();
    let resp_with_null = [resp_bytes, &[0u8]].concat(); // Add null terminator
    let resp_len = resp_with_null.len() as u64;
    let len_bytes = resp_len.to_le_bytes();

    // Send response length
    if let Err(e) = stream.write_all(&len_bytes) {
        if verbose {
            eprintln!("Error sending response length: {e}");
        }
        return;
    }

    // Send response with null terminator
    if let Err(e) = stream.write_all(&resp_with_null) {
        if verbose {
            eprintln!("Error sending response: {e}");
        }
        return;
    }

    if verbose {
        eprintln!("Sent response of length {}", resp_with_null.len());
    }
}
