//! Mock multipathd daemon for testing
//!
//! This daemon listens on an abstract namespace socket and responds to
//! multipath commands with test data from JSON files.

use clap::Parser;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::io::FromRawFd;
use std::path::PathBuf;
use std::mem;
use std::sync::{Arc, Mutex};
use std::thread;

use libc::{socket, bind, listen, accept, AF_UNIX, SOCK_STREAM, sockaddr_un, setsockopt, SOL_SOCKET, SO_REUSEADDR, close, getpid};

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
            eprintln!("Warning: Invalid file mapping format '{}', expected command=file[s]", mapping);
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
                        eprintln!("Loaded test data for '{}' from {} (custom mapping)", command, filepath.display());
                    }
                } else {
                    eprintln!("Warning: Could not load custom test data from {}", filepath.display());
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
                    eprintln!("Loaded test data for '{}' from {}", command, default_filepath.display());
                }
            }
            
            // Then try to load all other files in the subdirectory
            if let Ok(dir_entries) = fs::read_dir(cli.test_data_dir.join(subdir)) {
                let mut sorted_entries: Vec<_> = dir_entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map_or(false, |ft| ft.is_file()))
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
                                eprintln!("Loaded additional test data for '{}' from {}", command, filepath.display());
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
                        eprintln!("Loaded test data for '{}' from {}", command, flat_filepath.display());
                    }
                } else {
                    eprintln!("Warning: Could not load test data for '{}'", command);
                }
            }
        }
    }
    
    // Add a default response for unknown commands
    command_responses.entry("show maps json".to_string())
        .or_insert_with(|| vec![r#"{"major_version": 0, "minor_version": 1, "maps": []}"#.to_string()]);
    
    let file_counters = Arc::new(FileCounters::new());

    // Create socket
    let sock_fd = match create_abstract_socket(&cli.socket) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("Error creating socket: {}", e);
            std::process::exit(1);
        }
    };

    if cli.verbose {
        eprintln!("Listening on abstract namespace socket: {}", cli.socket);
        eprintln!("PID: {}", unsafe { getpid() });
    }

    // Listen for connections
    unsafe {
        if listen(sock_fd, 5) < 0 {
            eprintln!("Error listening on socket: {}", io::Error::last_os_error());
            close(sock_fd);
            std::process::exit(1);
        }
    }

    // Accept connections in a loop
    loop {
        let conn_fd = unsafe { accept(sock_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if conn_fd < 0 {
            eprintln!("Error accepting connection: {}", io::Error::last_os_error());
            continue;
        }

        if cli.verbose {
            eprintln!("Accepted connection");
        }

        // Spawn a thread to handle the connection
        let command_responses = command_responses.clone();
        let file_counters = file_counters.clone();
        thread::spawn(move || {
            handle_connection(conn_fd, command_responses, file_counters, cli.verbose);
        });
    }
}

/// Creates an abstract namespace socket and binds it
fn create_abstract_socket(socket_path: &str) -> io::Result<i32> {
    let fd = unsafe { socket(AF_UNIX, SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    // Set SO_REUSEADDR to allow quick restart
    let one: libc::c_int = 1;
    unsafe {
        setsockopt(
            fd,
            SOL_SOCKET,
            SO_REUSEADDR,
            &one as *const _ as *const libc::c_void,
            mem::size_of_val(&one) as libc::socklen_t,
        );
    }

    // Create abstract namespace socket address
    let mut addr: sockaddr_un = unsafe { mem::zeroed() };
    addr.sun_family = AF_UNIX as u16;
    
    // Extract the actual socket name, stripping the '@' prefix if present
    let socket_name = if socket_path.starts_with('@') {
        &socket_path[1..]
    } else {
        socket_path
    };
    
    let name_bytes = socket_name.as_bytes();
    if name_bytes.len() + 1 >= addr.sun_path.len() {
        unsafe { close(fd) };
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Socket path too long",
        ));
    }
    
    // For abstract namespace: sun_path[0] = '\0' and name starts at sun_path[1]
    addr.sun_path[0] = 0;
    for (i, &byte) in name_bytes.iter().enumerate() {
        addr.sun_path[1 + i] = byte as i8;
    }

    let addr_ptr = &addr as *const sockaddr_un;
    // Calculate the address length: sun_family (2) + 1 (null at sun_path[0]) + name_len
    let addr_len = 2 + 1 + name_bytes.len();

    let result = unsafe { bind(fd, addr_ptr as *const _, addr_len as libc::socklen_t) };
    if result < 0 {
        unsafe { close(fd) };
        return Err(io::Error::last_os_error());
    }

    Ok(fd)
}

/// Handles a single client connection
fn handle_connection(
    conn_fd: i32,
    command_responses: HashMap<String, Vec<String>>,
    file_counters: Arc<FileCounters>,
    verbose: bool,
) {
    // Read command length (8 bytes, little-endian)
    let mut len_bytes = [0u8; 8];
    let mut total_read = 0;

    while total_read < 8 {
        let mut fd_file = unsafe { std::fs::File::from_raw_fd(conn_fd) };
        match fd_file.read(&mut len_bytes[total_read..]) {
            Ok(0) => {
                std::mem::forget(fd_file);
                if verbose {
                    eprintln!("Connection closed while reading length");
                }
                unsafe { close(conn_fd) };
                return;
            }
            Ok(n) => {
                std::mem::forget(fd_file);
                total_read += n;
            }
            Err(e) => {
                std::mem::forget(fd_file);
                if verbose {
                    eprintln!("Error reading command length: {}", e);
                }
                unsafe { close(conn_fd) };
                return;
            }
        }
    }

    let cmd_len = u64::from_le_bytes(len_bytes) as usize;
    if verbose {
        eprintln!("Command length: {}", cmd_len);
    }

    // Read command
    let mut command_buf = vec![0u8; cmd_len];
    let mut total_read = 0;

    while total_read < cmd_len {
        let mut fd_file = unsafe { std::fs::File::from_raw_fd(conn_fd) };
        match fd_file.read(&mut command_buf[total_read..]) {
            Ok(0) => {
                std::mem::forget(fd_file);
                if verbose {
                    eprintln!("Connection closed while reading command");
                }
                unsafe { close(conn_fd) };
                return;
            }
            Ok(n) => {
                std::mem::forget(fd_file);
                total_read += n;
            }
            Err(e) => {
                std::mem::forget(fd_file);
                if verbose {
                    eprintln!("Error reading command: {}", e);
                }
                unsafe { close(conn_fd) };
                return;
            }
        }
    }

    // Convert to string (strip null terminator if present)
    let command = if let Some(pos) = command_buf.iter().position(|&b| b == 0) {
        String::from_utf8_lossy(&command_buf[..pos]).into_owned()
    } else {
        String::from_utf8_lossy(&command_buf).into_owned()
    };

    if verbose {
        eprintln!("Received command: '{}'", command);
    }

    // Look up response - now uses cycling through available files
    let response = if let Some(file_list) = command_responses.get(&command) {
        if file_list.len() == 1 {
            // Single file, no cycling needed
            file_list[0].clone()
        } else {
            // Multiple files, cycle through them
            let index = file_counters.next_index(&command, file_list.len());
            if verbose {
                eprintln!("Using file index {} for command '{}'", index, command);
            }
            file_list[index].clone()
        }
    } else if let Some(file_list) = command_responses.get("show maps json") {
        // Fallback to show maps json responses
        if file_list.len() == 1 {
            file_list[0].clone()
        } else {
            let index = file_counters.next_index("show maps json", file_list.len());
            file_list[index].clone()
        }
    } else {
        if verbose {
            eprintln!("No response for command '{}', using empty", command);
        }
        r#"{"error": "unknown command"}"#.to_string()
    };

    // Send response length (8 bytes, little-endian)
    let resp_bytes = response.as_bytes();
    let resp_with_null = [resp_bytes, &[0u8]].concat(); // Add null terminator
    let resp_len = resp_with_null.len() as u64;
    let len_bytes = resp_len.to_le_bytes();

    {
        let mut fd_file = unsafe { std::fs::File::from_raw_fd(conn_fd) };
        if let Err(e) = fd_file.write_all(&len_bytes) {
            if verbose {
                eprintln!("Error sending response length: {}", e);
            }
            std::mem::forget(fd_file);
            unsafe { close(conn_fd) };
            return;
        }
        std::mem::forget(fd_file);
    }

    // Send response with null terminator
    {
        let mut fd_file = unsafe { std::fs::File::from_raw_fd(conn_fd) };
        if let Err(e) = fd_file.write_all(&resp_with_null) {
            if verbose {
                eprintln!("Error sending response: {}", e);
            }
            std::mem::forget(fd_file);
            unsafe { close(conn_fd) };
            return;
        }
        std::mem::forget(fd_file);
    }

    if verbose {
        eprintln!("Sent response of length {}", resp_with_null.len());
    }

    unsafe { close(conn_fd) };
}
