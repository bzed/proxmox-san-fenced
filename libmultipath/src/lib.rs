//! Library for communicating with multipathd via its abstract namespace socket.
//!
//! This library provides functionality to connect to and communicate with
//! the multipathd daemon using the same protocol as the C library `libmpathcmd`.
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

use std::ffi::CString;
use std::io::{self, Read, Write};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Default socket path for multipathd (abstract namespace)
/// Note: For abstract namespace sockets, the '@' prefix is just a convention
/// in systemd. The actual socket name does not include the '@'.
pub const DEFAULT_SOCKET: &str = "@/org/kernel/linux/storage/multipathd";

/// Maximum reply length (32 MB, same as C implementation)
pub const MAX_REPLY_LEN: usize = 32 * 1024 * 1024;

/// Default reply timeout in milliseconds
pub const DEFAULT_REPLY_TIMEOUT_MS: u64 = 4000;

/// Default connection timeout in milliseconds
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 2000;

/// Represents a connection to the multipathd daemon.
pub struct MultipathConnection {
    stream: UnixStream,
}

impl MultipathConnection {
    /// Creates a new connection to multipathd using the default socket.
    pub fn new() -> io::Result<Self> {
        Self::with_socket(DEFAULT_SOCKET)
    }

    /// Creates a new connection to multipathd using the specified socket path.
    ///
    /// # Arguments
    ///
    /// * `socket_path` - The socket path to connect to (e.g., "@/org/kernel/linux/storage/multipathd")
    pub fn with_socket(socket_path: &str) -> io::Result<Self> {
        let stream = Self::connect_to_socket(socket_path)?;
        Ok(Self { stream })
    }

    /// Sends a command to multipathd and receives the reply.
    ///
    /// # Arguments
    ///
    /// * `command` - The command string to send (e.g., "show maps json")
    /// * `timeout_ms` - Optional timeout in milliseconds for the reply
    ///
    /// # Returns
    ///
    /// Returns the reply as a String on success.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use libmultipath::MultipathConnection;
    ///
    /// let conn = MultipathConnection::new().unwrap();
    /// let reply = conn.send_command("show maps json", Some(4000)).unwrap();
    /// println!("Reply: {}", reply);
    /// ```
    pub fn send_command(&self, command: &str, timeout_ms: Option<u64>) -> io::Result<String> {
        Self::send_command_on_stream(&self.stream, command, timeout_ms)
    }

    /// Sends a command to multipathd on the given UnixStream.
    ///
    /// # Arguments
    ///
    /// * `stream` - The UnixStream to use for communication
    /// * `command` - The command string to send
    /// * `timeout_ms` - Optional timeout in milliseconds for the reply
    ///
    /// # Returns
    ///
    /// Returns the reply as a String on success.
    pub fn send_command_on_stream(
        stream: &UnixStream,
        command: &str,
        timeout_ms: Option<u64>,
    ) -> io::Result<String> {
        // Set timeout if specified
        if let Some(timeout) = timeout_ms {
            let dur = Duration::from_millis(timeout);
            stream.set_read_timeout(Some(dur))?;
            stream.set_write_timeout(Some(dur))?;
        } else {
            stream.set_read_timeout(None)?;
            stream.set_write_timeout(None)?;
        }

        // Send command
        Self::send_command_stream(stream, command)?;

        // Receive reply
        Self::receive_reply_stream(stream)
    }

    /// Compatibility wrapper that sends a command to multipathd on the given file descriptor.
    ///
    /// # Arguments
    ///
    /// * `fd` - The file descriptor to use for communication
    /// * `command` - The command string to send
    /// * `timeout_ms` - Optional timeout in milliseconds for the reply
    ///
    /// # Returns
    ///
    /// Returns the reply as a String on success.
    pub fn send_command_on_fd(
        fd: i32,
        command: &str,
        timeout_ms: Option<u64>,
    ) -> io::Result<String> {
        if fd < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid file descriptor",
            ));
        }
        // SAFETY: The caller guarantees fd is valid. We temporarily wrap it in a
        // ManuallyDrop to prevent the stream destructor from closing it, even if a panic occurs.
        use std::os::fd::FromRawFd;
        use std::mem::ManuallyDrop;
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        let stream = ManuallyDrop::new(stream);
        Self::send_command_on_stream(&stream, command, timeout_ms)
    }

    /// Sends a command without receiving a reply.
    ///
    /// # Arguments
    ///
    /// * `command` - The command string to send
    pub fn send_command_no_reply(&self, command: &str) -> io::Result<()> {
        Self::send_command_stream(&self.stream, command)
    }

    fn connect_to_socket(socket_path: &str) -> io::Result<UnixStream> {
        let socket_path_clone = socket_path.to_string();
        let timeout = Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS);
        
        let (sender, receiver) = mpsc::channel();
        
        thread::spawn(move || {
            let result: io::Result<UnixStream> = if let Some(abstract_name) = socket_path_clone.strip_prefix('@') {
                match SocketAddr::from_abstract_name(abstract_name.as_bytes()) {
                    Ok(addr) => UnixStream::connect_addr(&addr),
                    Err(_) => Err(io::Error::new(io::ErrorKind::Other, "Invalid socket address")),
                }
            } else {
                UnixStream::connect(&socket_path_clone)
            };
            sender.send(result).ok();
        });
        
        match receiver.recv_timeout(timeout) {
            Ok(Ok(stream)) => Ok(stream),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Connection to {} timed out after {}ms", socket_path, DEFAULT_CONNECT_TIMEOUT_MS),
            )),
        }
    }

    fn send_command_stream(mut stream: &UnixStream, command: &str) -> io::Result<()> {
        let cmd_with_null =
            CString::new(command).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let cmd_bytes = cmd_with_null.as_bytes_with_nul();
        let cmd_len = cmd_bytes.len() as u64;

        stream.write_all(&cmd_len.to_le_bytes())?;
        stream.write_all(cmd_bytes)?;
        Ok(())
    }

    fn receive_reply_stream(mut stream: &UnixStream) -> io::Result<String> {
        let mut len_bytes = [0u8; 8];
        if let Err(e) = stream.read_exact(&mut len_bytes) {
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Timeout waiting for reply",
                ));
            }
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "Connection closed while reading length",
                ));
            }
            return Err(e);
        }

        let reply_len = u64::from_le_bytes(len_bytes) as usize;

        // Validate length
        if reply_len == 0 || reply_len > MAX_REPLY_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid reply length: {reply_len}"),
            ));
        }

        let mut reply_buf = vec![0u8; reply_len];
        if let Err(e) = stream.read_exact(&mut reply_buf) {
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Timeout waiting for reply data",
                ));
            }
            if e.kind() == io::ErrorKind::UnexpectedEof {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "Connection closed while reading data",
                ));
            }
            return Err(e);
        }

        if !reply_buf.is_empty() {
            let last_idx = reply_buf.len() - 1;
            reply_buf[last_idx] = 0;
        }

        if let Some(pos) = reply_buf.iter().position(|&b| b == 0) {
            reply_buf.truncate(pos);
        }

        String::from_utf8(reply_buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("Invalid UTF-8: {e}")))
    }
}

/// Convenience function to send a command to multipathd and get the reply.
///
/// Uses the default socket path and default timeout.
///
/// # Arguments
///
/// * `command` - The command string to send
///
/// # Returns
///
/// Returns the reply as a String on success.
///
/// # Examples
///
/// ```no_run
/// use libmultipath::send_multipath_command;
///
/// let reply = send_multipath_command("show maps json").unwrap();
/// println!("Reply: {}", reply);
/// ```
pub fn send_multipath_command(command: &str) -> io::Result<String> {
    let conn = MultipathConnection::new()?;
    conn.send_command(command, Some(DEFAULT_REPLY_TIMEOUT_MS))
}

/// Convenience function to send a command to multipathd with a custom timeout.
///
/// Uses the default socket path.
///
/// # Arguments
///
/// * `command` - The command string to send
/// * `timeout_ms` - Timeout in milliseconds
///
/// # Returns
///
/// Returns the reply as a String on success.
pub fn send_multipath_command_with_timeout(command: &str, timeout_ms: u64) -> io::Result<String> {
    let conn = MultipathConnection::new()?;
    conn.send_command(command, Some(timeout_ms))
}

/// Convenience function to send a command to a custom socket with default timeout.
///
/// # Arguments
///
/// * `socket_path` - The socket path to connect to
/// * `command` - The command string to send
///
/// # Returns
///
/// Returns the reply as a String on success.
pub fn send_multipath_command_to_socket(socket_path: &str, command: &str) -> io::Result<String> {
    let conn = MultipathConnection::with_socket(socket_path)?;
    conn.send_command(command, Some(DEFAULT_REPLY_TIMEOUT_MS))
}

/// Convenience function to send a command to a custom socket with custom timeout.
///
/// # Arguments
///
/// * `socket_path` - The socket path to connect to
/// * `command` - The command string to send
/// * `timeout_ms` - Timeout in milliseconds
///
/// # Returns
///
/// Returns the reply as a String on success.
pub fn send_multipath_command_to_socket_with_timeout(
    socket_path: &str,
    command: &str,
    timeout_ms: u64,
) -> io::Result<String> {
    let conn = MultipathConnection::with_socket(socket_path)?;
    conn.send_command(command, Some(timeout_ms))
}
