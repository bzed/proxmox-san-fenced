//! pve-san-fenced: SAN fencing daemon for Proxmox VE
//!
//! Continuously monitors multipath storage states and writes to the kernel
//! SysRq trigger upon complete, persistent storage loss for LUNs actively
//! used by running VMs.
//!
//! Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
//!
//! This program is free software: you can redistribute it and/or modify
//! it under the terms of the GNU Affero General Public License as published by
//! the Free Software Foundation, either version 3 of the License, or
//! (at your option) any later version.

use clap::Parser;
use log::{debug, error, info, warn};
use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use pve_san_fenced::{discover_in_use_mpaths, trigger_fencing, Fencer};

/// Holds active multipath devices along with their discovery timestamp
#[derive(Debug, Clone)]
struct ActiveLunsWithTimestamp {
    luns: HashSet<String>,
    discovered_at: Instant,
}

impl ActiveLunsWithTimestamp {
    fn new(luns: HashSet<String>) -> Self {
        Self {
            luns,
            discovered_at: Instant::now(),
        }
    }

    /// Check if the data is too stale to use
    /// Data is considered stale if it's older than 2x the discovery interval
    fn is_stale(&self, discovery_interval: Duration) -> bool {
        let max_age = discovery_interval * 2;
        self.discovered_at.elapsed() > max_age
    }
}

fn get_default_node_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "localhost".to_string())
}

/// SAN fencing daemon for Proxmox VE
#[derive(Parser, Debug, PartialEq)]
#[command(name = "pve-san-fenced")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "SAN fencing daemon for Proxmox VE", long_about = None)]
struct Cli {
    /// Seconds between multipathd checks
    #[arg(long, env = "PVE_SAN_POLL_INTERVAL", default_value = "5")]
    poll_interval: u64,

    /// Seconds between VM and storage discovery scans
    #[arg(long, env = "PVE_SAN_DISCOVERY_INTERVAL", default_value = "60")]
    discovery_interval: u64,

    /// Number of consecutive failures before fencing
    #[arg(long, env = "PVE_SAN_MAX_FAILURES", default_value = "6")]
    max_failures: u64,

    /// Specific WWIDs to monitor (if empty, monitors all maps in use by running VMs)
    #[arg(long)]
    target_wwids: Vec<String>,

    /// Multipath socket to connect to
    #[arg(long, env = "PVE_SAN_SOCKET", default_value = libmultipath::DEFAULT_SOCKET)]
    socket: String,

    /// The name of the local Proxmox node
    #[arg(
        long = "node-name",
        short = 'n',
        env = "PVE_SAN_NODE_NAME",
        default_value_t = get_default_node_name()
    )]
    node_name: String,

    /// Command to use for Proxmox VE API queries
    #[arg(long, env = "PVE_SAN_PVESH_COMMAND", default_value = "pvesh")]
    pvesh_command: String,

    /// Run in test mode (only logs changes and decisions, does not trigger reboot)
    #[arg(long, short = 't', env = "PVE_SAN_TEST_MODE")]
    test_mode: bool,

    /// The character(s) to write to /proc/sysrq-trigger (default: s,b for sync followed by reboot)
    #[arg(
        long = "sysrq-char",
        alias = "sysrq-chars",
        env = "PVE_SAN_SYSRQ_CHAR",
        default_value = "s,b"
    )]
    sysrq_char: String,

    /// Query and print the daemon status from the status file, exiting with the corresponding Nagios exit code
    #[arg(long)]
    status: bool,

    /// Path to write Nagios-compatible status file
    #[arg(
        long = "status-file",
        env = "PVE_SAN_STATUS_FILE",
        default_value = "/run/pve-san-fenced/status"
    )]
    status_file: String,

    /// Enable debug log mode to log discovered VMs, storages, and multipath devices with their state on each discovery run
    #[arg(long, env = "PVE_SAN_DEBUG")]
    debug: bool,

    /// Maximum number of consecutive discovery failures before applying backoff (0 = no backoff)
    #[arg(long, env = "PVE_SAN_DISCOVERY_MAX_RETRIES", default_value = "5")]
    discovery_max_retries: u64,

    /// Base delay in seconds for exponential backoff
    #[arg(long, env = "PVE_SAN_DISCOVERY_BACKOFF_BASE", default_value = "1")]
    discovery_backoff_base: u64,

    /// Maximum backoff delay in seconds
    #[arg(long, env = "PVE_SAN_DISCOVERY_BACKOFF_MAX", default_value = "30")]
    discovery_backoff_max: u64,
}


fn sysrq_char_to_bit(c: char) -> Option<i32> {
    match c {
        's' => Some(16),
        'b' | 'o' => Some(128),
        'c' => Some(2),
        'u' => Some(32),
        'r' => Some(4),
        'e' | 'i' | 'f' => Some(64),
        't' | 'p' | 'm' | 'w' => Some(8),
        _ => None,
    }
}

fn validate_sysrq(sysrq_chars: &str) -> Result<(), String> {
    let chars: Vec<char> = sysrq_chars
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .flat_map(str::chars)
        .collect();

    if chars.is_empty() {
        return Err("sysrq-char configuration cannot be empty".to_string());
    }

    for &c in &chars {
        if sysrq_char_to_bit(c).is_none() {
            return Err(format!(
                "Invalid SysRq character '{c}' specified in configuration"
            ));
        }
    }

    if std::env::var("PVE_SAN_FENCE_DRY_RUN").is_ok() {
        return Ok(());
    }

    let sysrq_path = "/proc/sys/kernel/sysrq";
    match std::fs::read_to_string(sysrq_path) {
        Ok(content) => {
            let val_str = content.trim();
            match val_str.parse::<i32>() {
                Ok(val) => {
                    if val == 0 {
                        let msg = format!("SysRq is disabled (value is 0) in {sysrq_path}. Fencing operations will fail!");
                        warn!("CRITICAL: {msg}");
                        pve_san_fenced::status::get_status_tracker().set_issue(
                            "sysrq",
                            pve_san_fenced::status::StatusLevel::Warning,
                            msg,
                        );
                    } else if val != 1 {
                        for c in chars {
                            if let Some(bit) = sysrq_char_to_bit(c) {
                                if (val & bit) == 0 {
                                    let msg = format!("Configured SysRq char '{c}' is disabled by {sysrq_path} bitmask ({val_str})!");
                                    warn!("CRITICAL: {msg}");
                                    pve_san_fenced::status::get_status_tracker().set_issue(
                                        &format!("sysrq_{c}"),
                                        pve_san_fenced::status::StatusLevel::Warning,
                                        msg,
                                    );
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    let msg = format!("Could not parse {sysrq_path} value: {val_str}");
                    warn!("{msg}");
                    pve_san_fenced::status::get_status_tracker().set_issue(
                        "sysrq_parse",
                        pve_san_fenced::status::StatusLevel::Warning,
                        msg,
                    );
                }
            }
        }
        Err(e) => {
            let msg = format!("Failed to read {sysrq_path}: {e}. Unable to verify SysRq state.");
            warn!("{msg}");
            pve_san_fenced::status::get_status_tracker().set_issue(
                "sysrq_read",
                pve_san_fenced::status::StatusLevel::Warning,
                msg,
            );
        }
    }

    Ok(())
}

fn exit_with_flush(code: i32) -> ! {
    // Wait briefly to allow the status file write thread to write status file
    std::thread::sleep(std::time::Duration::from_millis(200));
    std::process::exit(code);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Set default log level to info if not set
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let cli = Cli::parse();
    if cli.status {
        let path = &cli.status_file;
        // Check if file is outdated (modified time exceeds threshold)
        if let Ok(metadata) = std::fs::metadata(path) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    let threshold = std::cmp::max(30, 3 * cli.poll_interval);
                    if elapsed.as_secs() > threshold {
                        println!(
                            "UNKNOWN - Status file is outdated (last modified {} seconds ago)",
                            elapsed.as_secs()
                        );
                        std::process::exit(3);
                    }
                }
            }
        }

        match std::fs::read_to_string(path) {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    println!("UNKNOWN - Status file is empty");
                    std::process::exit(3);
                }
                if trimmed.starts_with("OK -") {
                    println!("{trimmed}");
                    std::process::exit(0);
                } else if trimmed.starts_with("WARNING -") {
                    println!("{trimmed}");
                    std::process::exit(1);
                } else if trimmed.starts_with("CRITICAL -") {
                    println!("{trimmed}");
                    std::process::exit(2);
                } else {
                    println!("UNKNOWN - Badly formatted status file: {trimmed}");
                    std::process::exit(3);
                }
            }
            Err(e) => {
                println!("UNKNOWN - Failed to read status file '{path}': {e}");
                std::process::exit(3);
            }
        }
    }

    // Set the status file immediately so that errors/warnings are reported
    pve_san_fenced::status::get_status_tracker().set_status_file(Some(cli.status_file.clone()));

    if cli.poll_interval == 0 {
        let msg = "poll-interval cannot be 0".to_string();
        error!("{msg}");
        pve_san_fenced::status::get_status_tracker().set_issue(
            "config_error",
            pve_san_fenced::status::StatusLevel::Critical,
            msg,
        );
        exit_with_flush(1);
    }
    if cli.max_failures == 0 {
        let msg = "max-failures cannot be 0".to_string();
        error!("{msg}");
        pve_san_fenced::status::get_status_tracker().set_issue(
            "config_error",
            pve_san_fenced::status::StatusLevel::Critical,
            msg,
        );
        exit_with_flush(1);
    }
    if cli.discovery_interval == 0 {
        let msg = "discovery-interval cannot be 0".to_string();
        error!("{msg}");
        pve_san_fenced::status::get_status_tracker().set_issue(
            "config_error",
            pve_san_fenced::status::StatusLevel::Critical,
            msg,
        );
        exit_with_flush(1);
    }
    let base_dir =
        std::env::var("PVE_SAN_SYS_NODES_DIR").unwrap_or_else(|_| "/etc/pve/nodes".to_string());
    let node_dir = std::path::Path::new(&base_dir).join(&cli.node_name);
    if !node_dir.is_dir() {
        let display_path = node_dir.display();
        let msg = format!("Node directory '{display_path}' does not exist under {base_dir}");
        error!("{msg}");
        pve_san_fenced::status::get_status_tracker().set_issue(
            "config_error",
            pve_san_fenced::status::StatusLevel::Critical,
            msg,
        );
        exit_with_flush(1);
    }
    let node = &cli.node_name;
    info!("Starting PVE SAN fencing daemon on node: {node}");

    if let Err(e) = validate_sysrq(&cli.sysrq_char) {
        let msg = format!("Configuration error: {e}");
        error!("{msg}");
        pve_san_fenced::status::get_status_tracker().set_issue(
            "config_error",
            pve_san_fenced::status::StatusLevel::Critical,
            msg,
        );
        exit_with_flush(1);
    }

    let discovery_interval = cli.discovery_interval;
    let discovery_interval_duration = Duration::from_secs(discovery_interval);
    let active_luns = Arc::new(RwLock::new(ActiveLunsWithTimestamp::new(HashSet::new())));

    // Spawn VM and storage discovery task in an independent OS thread
    let active_luns_clone = Arc::clone(&active_luns);
    let node_clone = cli.node_name.clone();
    let pvesh_cmd_clone = cli.pvesh_command.clone();
    let socket_clone = cli.socket.clone();
    let debug_mode = cli.debug;
    let max_retries = cli.discovery_max_retries;
    let backoff_base = cli.discovery_backoff_base;
    let backoff_max = cli.discovery_backoff_max;

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for discovery thread");

        rt.block_on(async {
            let mut consecutive_failures = 0u64;
            loop {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    discover_in_use_mpaths(
                        &node_clone,
                        &pvesh_cmd_clone,
                        Some(&socket_clone),
                        debug_mode,
                    )
                }));

                match result {
                    Ok(fut) => {
                        match fut.await {
                            Ok(mpaths) => {
                                consecutive_failures = 0;
                                pve_san_fenced::status::get_status_tracker().clear_issue("discovery_failure");
                                pve_san_fenced::status::get_status_tracker().clear_issue("discovery_backoff");
                                let mut lock = active_luns_clone.write().await;
                                if lock.luns != mpaths {
                                    let prev = &lock.luns;
                                    info!("Active multipath devices changed. Previous: {prev:?}, New: {mpaths:?}");
                                }
                                *lock = ActiveLunsWithTimestamp::new(mpaths);
                            }
                            Err(e) => {
                                consecutive_failures += 1;
                                let msg = format!("Error discovering active multipath devices: {e}");
                                error!("{msg}");
                                pve_san_fenced::status::get_status_tracker().set_issue(
                                    "discovery_failure",
                                    pve_san_fenced::status::StatusLevel::Warning,
                                    msg,
                                );
                            }
                        }
                    }
                    Err(panic_err) => {
                        consecutive_failures += 1;
                        let error_msg = if let Some(s) = panic_err.downcast_ref::<&str>() {
                            s.to_string()
                        } else if let Some(s) = panic_err.downcast_ref::<String>() {
                            s.clone()
                        } else {
                            "Unknown panic error".to_string()
                        };
                        let msg = format!("Panic in discovery thread: {error_msg}");
                        error!("{msg}");
                        pve_san_fenced::status::get_status_tracker().set_issue(
                            "discovery_failure",
                            pve_san_fenced::status::StatusLevel::Warning,
                            msg,
                        );
                    }
                }

                // Apply exponential backoff if we have consecutive failures and max_retries > 0
                if consecutive_failures > 0 && max_retries > 0 {
                    if consecutive_failures >= max_retries {
                        // Calculate backoff delay with exponential growth, capped at backoff_max
                        let backoff_exp = consecutive_failures.saturating_sub(max_retries);
                        let backoff_seconds = std::cmp::min(
                            backoff_base.saturating_mul(2u64.pow(backoff_exp as u32)),
                            backoff_max,
                        );
                        let msg = format!(
                            "Discovery thread: {consecutive_failures} consecutive failures, backing off for {backoff_seconds} seconds"
                        );
                        warn!("{msg}");
                        pve_san_fenced::status::get_status_tracker().set_issue(
                            "discovery_backoff",
                            pve_san_fenced::status::StatusLevel::Warning,
                            msg,
                        );
                        tokio::time::sleep(Duration::from_secs(backoff_seconds)).await;
                    } else {
                        // Normal interval
                        tokio::time::sleep(Duration::from_secs(discovery_interval)).await;
                    }
                } else {
                    // Normal interval
                    tokio::time::sleep(Duration::from_secs(discovery_interval)).await;
                }
            }
        });
    });

    // Run multipath monitoring loop
    let socket = cli.socket.clone();
    let target_wwids: HashSet<String> = cli.target_wwids.into_iter().collect();
    let poll_interval = cli.poll_interval;
    let max_failures = cli.max_failures;
    let test_mode = cli.test_mode;

    let mut fencer = Fencer::new(max_failures, target_wwids);

    let mut interval = tokio::time::interval(Duration::from_secs(poll_interval));
    loop {
        interval.tick().await;

        // Keep the status file fresh on every cycle
        pve_san_fenced::status::get_status_tracker().touch();

        let cf = fencer.consecutive_failures();
        let mf = fencer.max_failures();
        debug!("Fencer monitoring state: consecutive_failures={cf}, max_failures={mf}");

        // Read active LUNs with timestamp
        let active_data = {
            let lock = active_luns.read().await;
            lock.clone()
        };

        // Check if the data is too stale to use
        if active_data.is_stale(discovery_interval_duration) {
            let msg = "Active LUN data is stale (older than 2x discovery interval). Skipping fencer update to avoid race condition with discovery thread.".to_string();
            warn!("{msg}");
            pve_san_fenced::status::get_status_tracker().set_issue(
                "stale_luns",
                pve_san_fenced::status::StatusLevel::Warning,
                msg,
            );
            continue;
        } else {
            pve_san_fenced::status::get_status_tracker().clear_issue("stale_luns");
        }

        let active_set = active_data.luns;
        debug!("Current active LUNs set: {active_set:?}");

        // Query multipathd
        let response =
            match libmultipath::send_multipath_command_to_socket(&socket, "show maps json") {
                Ok(res) => {
                    pve_san_fenced::status::get_status_tracker().clear_issue("query_failure");
                    res
                }
                Err(e) => {
                    let msg = format!("Failed to query multipathd: {e}");
                    warn!("{msg}");
                    pve_san_fenced::status::get_status_tracker().set_issue(
                        "query_failure",
                        pve_san_fenced::status::StatusLevel::Warning,
                        msg,
                    );
                    // Incrementing consecutive failures here could trigger reboot on transient daemon restarts.
                    // We just log warning as per the specification.
                    continue;
                }
            };

        let maps = if let Some(m) = pve_san_fenced::parse_multipathd_response(&response) {
            m
        } else {
            // Parsing failed, skip fencer update to avoid triggering on bad JSON
            continue;
        };

        match libmultipath::send_multipath_command_to_socket(&socket, "show config local") {
            Ok(config_response) => {
                pve_san_fenced::status::get_status_tracker().clear_issue("config_query_error");
                pve_san_fenced::config::check_maps_config(&maps, &active_set, &config_response);
            }
            Err(e) => {
                let msg = format!("Failed to query multipathd config: {e}");
                warn!("{msg}");
                pve_san_fenced::status::get_status_tracker().set_issue(
                    "config_query_error",
                    pve_san_fenced::status::StatusLevel::Warning,
                    msg,
                );
            }
        }

        if fencer.update_with_maps(&maps, &active_set) {
            if test_mode {
                info!("TEST MODE: Fencing decision reached, but not executing reboot/SysRq kernel panic.");
            } else {
                trigger_fencing(&cli.sysrq_char).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sysrq_char_to_bit() {
        assert_eq!(sysrq_char_to_bit('s'), Some(16));
        assert_eq!(sysrq_char_to_bit('b'), Some(128));
        assert_eq!(sysrq_char_to_bit('o'), Some(128));
        assert_eq!(sysrq_char_to_bit('c'), Some(2));
        assert_eq!(sysrq_char_to_bit('u'), Some(32));
        assert_eq!(sysrq_char_to_bit('r'), Some(4));
        assert_eq!(sysrq_char_to_bit('e'), Some(64));
        assert_eq!(sysrq_char_to_bit('i'), Some(64));
        assert_eq!(sysrq_char_to_bit('f'), Some(64));
        assert_eq!(sysrq_char_to_bit('t'), Some(8));
        assert_eq!(sysrq_char_to_bit('p'), Some(8));
        assert_eq!(sysrq_char_to_bit('m'), Some(8));
        assert_eq!(sysrq_char_to_bit('w'), Some(8));
        assert_eq!(sysrq_char_to_bit('x'), None);
    }

    #[test]
    fn test_validate_sysrq_characters() {
        // Valid scenarios
        assert!(validate_sysrq("s,b").is_ok());
        assert!(validate_sysrq("c").is_ok());
        assert!(validate_sysrq("s,b,u").is_ok());

        // Invalid scenarios
        assert!(validate_sysrq("x").is_err());
        assert!(validate_sysrq("s,b,x").is_err());
        assert!(validate_sysrq("s,b,@").is_err());
        assert!(validate_sysrq("").is_err());
        assert!(validate_sysrq(",,").is_err());

        // With PVE_SAN_FENCE_DRY_RUN set
        struct LocalEnvGuard(Option<String>);
        impl Drop for LocalEnvGuard {
            fn drop(&mut self) {
                if let Some(val) = &self.0 {
                    std::env::set_var("PVE_SAN_FENCE_DRY_RUN", val);
                } else {
                    std::env::remove_var("PVE_SAN_FENCE_DRY_RUN");
                }
            }
        }
        let _guard = LocalEnvGuard(std::env::var("PVE_SAN_FENCE_DRY_RUN").ok());
        std::env::set_var("PVE_SAN_FENCE_DRY_RUN", "1");

        // Invalid scenarios should still fail under dry-run
        assert!(validate_sysrq("x").is_err());
        assert!(validate_sysrq("s,b,x").is_err());
        assert!(validate_sysrq("s,b,@").is_err());
    }

    #[test]
    fn test_cli_parsing() {
        let args = vec!["pve-san-fenced", "-n", "pve01", "-t", "--sysrq-char", "c"];
        let cli = Cli::try_parse_from(args).unwrap();
        let expected = Cli {
            poll_interval: 5,
            discovery_interval: 60,
            max_failures: 6,
            target_wwids: vec![],
            socket: libmultipath::DEFAULT_SOCKET.to_string(),
            node_name: "pve01".to_string(),
            pvesh_command: "pvesh".to_string(),
            test_mode: true,
            sysrq_char: "c".to_string(),
            status: false,
            status_file: "/run/pve-san-fenced/status".to_string(),
            debug: false,
            discovery_max_retries: 5,
            discovery_backoff_base: 1,
            discovery_backoff_max: 30,
        };
        assert_eq!(cli, expected);

        let args_alias = vec![
            "pve-san-fenced",
            "-n",
            "pve01",
            "-t",
            "--sysrq-chars",
            "s,b,c",
        ];
        let cli_alias = Cli::try_parse_from(args_alias).unwrap();
        assert_eq!(cli_alias.sysrq_char, "s,b,c");

        let args_no_sysrq = vec!["pve-san-fenced", "-n", "pve01", "-t"];
        let cli_no_sysrq = Cli::try_parse_from(args_no_sysrq).unwrap();
        assert_eq!(cli_no_sysrq.sysrq_char, "s,b");

        let args2 = vec![
            "pve-san-fenced",
            "-n",
            "pve01",
            "--test-mode",
            "--sysrq-char",
            "c",
        ];
        let cli2 = Cli::try_parse_from(args2).unwrap();
        assert_eq!(cli2, expected);

        let args_default = vec!["pve-san-fenced", "-t", "--sysrq-char", "c"];
        let cli_default = Cli::try_parse_from(args_default).unwrap();
        let expected_default = Cli {
            poll_interval: 5,
            discovery_interval: 60,
            max_failures: 6,
            target_wwids: vec![],
            socket: libmultipath::DEFAULT_SOCKET.to_string(),
            node_name: get_default_node_name(),
            pvesh_command: "pvesh".to_string(),
            test_mode: true,
            sysrq_char: "c".to_string(),
            status: false,
            status_file: "/run/pve-san-fenced/status".to_string(),
            debug: false,
            discovery_max_retries: 5,
            discovery_backoff_base: 1,
            discovery_backoff_max: 30,
        };
        assert_eq!(cli_default, expected_default);

        // Test environment variable overrides
        struct EnvGuard {
            saved_vars: Vec<(String, Option<String>)>,
        }

        impl EnvGuard {
            fn new(keys: &[&str]) -> Self {
                let mut saved_vars = Vec::new();
                for key in keys {
                    let val = std::env::var(key).ok();
                    saved_vars.push((key.to_string(), val));
                }
                Self { saved_vars }
            }
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (key, val) in &self.saved_vars {
                    if let Some(v) = val {
                        std::env::set_var(key, v);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
        }

        let _guard = EnvGuard::new(&[
            "PVE_SAN_POLL_INTERVAL",
            "PVE_SAN_MAX_FAILURES",
            "PVE_SAN_TEST_MODE",
        ]);
        std::env::set_var("PVE_SAN_POLL_INTERVAL", "15");
        std::env::set_var("PVE_SAN_MAX_FAILURES", "10");
        std::env::set_var("PVE_SAN_TEST_MODE", "true");

        let args_env = vec!["pve-san-fenced", "-n", "pve01", "--sysrq-char", "c"];
        let cli_env = Cli::try_parse_from(args_env).unwrap();

        assert_eq!(cli_env.poll_interval, 15);
        assert_eq!(cli_env.max_failures, 10);
        assert!(cli_env.test_mode);
    }}
