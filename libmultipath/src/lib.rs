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
use std::io;
use std::mem;

use libc::{
    socket, connect, AF_UNIX, SOCK_STREAM, sockaddr_un, SOL_SOCKET, SO_RCVTIMEO,
    SO_SNDTIMEO, setsockopt, timeval, close,
};

/// Default socket path for multipathd (abstract namespace)
/// Note: For abstract namespace sockets, the '@' prefix is just a convention
/// in systemd. The actual socket name does not include the '@'.
pub const DEFAULT_SOCKET: &str = "/org/kernel/linux/storage/multipathd";

/// Maximum reply length (32 MB, same as C implementation)
pub const MAX_REPLY_LEN: usize = 32 * 1024 * 1024;

/// Default reply timeout in milliseconds
pub const DEFAULT_REPLY_TIMEOUT_MS: u64 = 4000;

/// Represents a connection to the multipathd daemon.
pub struct MultipathConnection {
    fd: i32,
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
        let fd = Self::connect_to_socket(socket_path)?;
        Ok(Self { fd })
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
    pub fn send_command(&self, command: &str, timeout_ms: Option<u64>) -> io::Result<String> {
        Self::send_command_on_fd(self.fd, command, timeout_ms)
    }

    /// Sends a command to multipathd on the given file descriptor.
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
    pub fn send_command_on_fd(fd: i32, command: &str, timeout_ms: Option<u64>) -> io::Result<String> {
        // Validate fd is non-negative
        if fd < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid file descriptor",
            ));
        }

        // Send command
        Self::send_command_fd(fd, command)?;

        // Set timeout if specified
        if let Some(timeout) = timeout_ms {
            Self::set_socket_timeout(fd, timeout)?;
        }

        // Receive reply
        Self::receive_reply(fd)
    }

    /// Sends a command without receiving a reply.
    ///
    /// # Arguments
    ///
    /// * `command` - The command string to send
    pub fn send_command_no_reply(&self, command: &str) -> io::Result<()> {
        Self::send_command_fd(self.fd, command)
    }

    fn set_socket_timeout(fd: i32, timeout_ms: u64) -> io::Result<()> {
        let timeout_sec = (timeout_ms / 1000) as libc::time_t;
        let timeout_usec = ((timeout_ms % 1000) * 1000) as libc::suseconds_t;

        let timeout_val = timeval {
            tv_sec: timeout_sec,
            tv_usec: timeout_usec,
        };

        unsafe {
            let result = setsockopt(
                fd,
                SOL_SOCKET,
                SO_RCVTIMEO,
                &timeout_val as *const _ as *const libc::c_void,
                mem::size_of_val(&timeout_val) as libc::socklen_t,
            );
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
            let result = setsockopt(
                fd,
                SOL_SOCKET,
                SO_SNDTIMEO,
                &timeout_val as *const _ as *const libc::c_void,
                mem::size_of_val(&timeout_val) as libc::socklen_t,
            );
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(())
    }

    fn connect_to_socket(socket_path: &str) -> io::Result<i32> {
        let fd = unsafe { socket(AF_UNIX, SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // Create abstract namespace socket address
        let mut addr: sockaddr_un = unsafe { mem::zeroed() };
        addr.sun_family = AF_UNIX as u16;
        // For abstract namespace: sun_path[0] = '\0' and name starts at sun_path[1]
        addr.sun_path[0] = 0;

        // Extract the actual socket name, stripping the '@' prefix if present
        // (the '@' is just a convention in systemd, not part of the actual name)
        let socket_name = socket_path.strip_prefix('@').unwrap_or(socket_path);

        let name_bytes = socket_name.as_bytes();
        let name_len = name_bytes.len();
        if name_len + 1 >= addr.sun_path.len() {
            unsafe { close(fd) };
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Socket path too long",
            ));
        }
        // Copy bytes to sun_path (which is [i8; 108])
        for (i, &byte) in name_bytes.iter().enumerate() {
            addr.sun_path[1 + i] = byte as i8;
        }

        let addr_ptr = &addr as *const sockaddr_un;
        // Calculate the address length matching the C code:
        // len = strlen(socket_name) + 1 + sizeof(sa_family_t)
        // where the +1 accounts for the null byte at sun_path[0]
        let addr_len = name_len + 1 + 2; // name_len + 1 (null at sun_path[0]) + 2 (sun_family)

        let result = unsafe { connect(fd, addr_ptr as *const _, addr_len as libc::socklen_t) };
        if result < 0 {
            unsafe { close(fd) };
            return Err(io::Error::last_os_error());
        }

        Ok(fd)
    }

    fn send_command_fd(fd: i32, command: &str) -> io::Result<()> {
        // Command string with null terminator
        let cmd_with_null = CString::new(command)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let cmd_bytes = cmd_with_null.as_bytes_with_nul();
        let cmd_len = cmd_bytes.len() as u64;

        // Send length (8 bytes, little-endian) using raw fd
        let len_bytes = cmd_len.to_le_bytes();
        let result = unsafe {
            libc::write(
                fd,
                len_bytes.as_ptr() as *const libc::c_void,
                len_bytes.len() as libc::size_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }

        // Send command using raw fd
        let result = unsafe {
            libc::write(
                fd,
                cmd_bytes.as_ptr() as *const libc::c_void,
                cmd_bytes.len() as libc::size_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(())
    }

    fn receive_reply(fd: i32) -> io::Result<String> {
        // Receive length (8 bytes)
        let mut len_bytes = [0u8; 8];
        let mut total_read = 0;

        // Read exactly 8 bytes for the length using raw fd
        while total_read < 8 {
            let result = unsafe {
                libc::read(
                    fd,
                    len_bytes[total_read..].as_mut_ptr() as *mut libc::c_void,
                    (8 - total_read) as libc::size_t,
                )
            };
            match result {
                -1 => {
                    let err = io::Error::last_os_error();
                    // If timeout, return timeout error
                    if err.raw_os_error() == Some(libc::EAGAIN) || err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Timeout waiting for reply",
                        ));
                    }
                    return Err(err);
                }
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "Connection closed while reading length",
                    ));
                }
                n => {
                    total_read += n as usize;
                }
            }
        }

        let reply_len = u64::from_le_bytes(len_bytes) as usize;

        // Validate length - the C code checks: len <= 0 || len >= MAX_REPLY_LEN
        // We use > instead of >= to allow exactly MAX_REPLY_LEN bytes
        if reply_len == 0 || reply_len > MAX_REPLY_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid reply length: {}", reply_len),
            ));
        }

        // Receive reply data
        let mut reply_buf = vec![0u8; reply_len];
        let mut total_read = 0;

        while total_read < reply_len {
            let result = unsafe {
                libc::read(
                    fd,
                    reply_buf[total_read..].as_mut_ptr() as *mut libc::c_void,
                    (reply_len - total_read) as libc::size_t,
                )
            };
            match result {
                -1 => {
                    let err = io::Error::last_os_error();
                    // If timeout, return timeout error
                    if err.raw_os_error() == Some(libc::EAGAIN) || err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Timeout waiting for reply data",
                        ));
                    }
                    return Err(err);
                }
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "Connection closed while reading data",
                    ));
                }
                n => {
                    total_read += n as usize;
                }
            }
        }

        // The C code does: reply[len - 1] = '\\0'
        // This ensures the string is null-terminated.
        if !reply_buf.is_empty() {
            let last_idx = reply_buf.len() - 1;
            reply_buf[last_idx] = 0;
        }

        // Convert to String, excluding any null bytes at the end
        if let Some(pos) = reply_buf.iter().position(|&b| b == 0) {
            reply_buf.truncate(pos);
        }

        String::from_utf8(reply_buf).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("Invalid UTF-8: {}", e))
        })
    }
}

impl Drop for MultipathConnection {
    fn drop(&mut self) {
        unsafe { close(self.fd) };
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
