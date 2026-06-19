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

use std::collections::HashSet;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use clap::Parser;
use log::{debug, error, info, warn};
use tokio::sync::RwLock;

use pve_san_fenced::{
    discover_in_use_mpaths, trigger_fencing, Fencer,
};

/// SAN fencing daemon for Proxmox VE
#[derive(Parser, Debug)]
#[command(name = "pve-san-fenced")]
#[command(author = "PVE SAN Fenced")]
#[command(version = "0.1.0")]
#[command(about = "SAN fencing daemon for Proxmox VE", long_about = None)]
struct Cli {
    /// Seconds between multipathd checks
    #[arg(long, default_value = "5")]
    poll_interval: u64,

    /// Seconds between VM and storage discovery scans
    #[arg(long, default_value = "60")]
    discovery_interval: u64,

    /// Number of consecutive failures before fencing
    #[arg(long, default_value = "6")]
    max_failures: u64,

    /// Specific WWIDs to monitor (if empty, monitors all maps in use by running VMs)
    #[arg(long)]
    target_wwids: Vec<String>,

    /// Multipath socket to connect to
    #[arg(long, default_value = libmultipath::DEFAULT_SOCKET)]
    socket: String,

    /// The name of the local Proxmox node
    #[arg(long, short = 'n')]
    node: String,

    /// Command to use for Proxmox VE API queries
    #[arg(long, default_value = "pvesh")]
    pvesh_command: String,

    /// Run in test mode (only logs changes and decisions, does not trigger reboot)
    #[arg(long, short = 't')]
    test_mode: bool,

    /// The character to write to /proc/sysrq-trigger (default: b for immediate reboot, c for panic)
    #[arg(long, default_value = "b")]
    sysrq_char: String,
}

fn validate_sysrq(sysrq_char: &str) {
    if std::env::var("PVE_SAN_FENCE_DRY_RUN").is_ok() {
        return;
    }

    let sysrq_path = "/proc/sys/kernel/sysrq";
    match std::fs::read_to_string(sysrq_path) {
        Ok(content) => {
            let val_str = content.trim();
            match val_str.parse::<i32>() {
                Ok(val) => {
                    if val == 0 {
                        warn!("CRITICAL: SysRq is disabled (value is 0) in {sysrq_path}. Fencing operations will fail!");
                    } else {
                        let allowed = match sysrq_char {
                            "b" => val == 1 || (val & 128) != 0,
                            "c" => val == 1 || (val & 2) != 0,
                            _ => val == 1,
                        };
                        if !allowed {
                            warn!("CRITICAL: Configured SysRq char '{sysrq_char}' might be disabled by {sysrq_path} bitmask ({val_str})!");
                        }
                    }
                }
                Err(_) => {
                    warn!("Could not parse {sysrq_path} value: {val_str}");
                }
            }
        }
        Err(e) => {
            warn!("Failed to read {sysrq_path}: {e}. Unable to verify SysRq state.");
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Set default log level to info if not set
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }
    env_logger::init();

    let cli = Cli::parse();
    info!("Starting PVE SAN fencing daemon on node: {}", cli.node);

    validate_sysrq(&cli.sysrq_char);

    let active_luns = Arc::new(RwLock::new(HashSet::new()));

    // Spawn VM and storage discovery task in an independent OS thread
    let active_luns_clone = Arc::clone(&active_luns);
    let node_clone = cli.node.clone();
    let pvesh_cmd_clone = cli.pvesh_command.clone();
    let discovery_interval = cli.discovery_interval;

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime for discovery thread");

        rt.block_on(async {
            loop {
                match discover_in_use_mpaths(&node_clone, &pvesh_cmd_clone).await {
                    Ok(mpaths) => {
                        let mut lock = active_luns_clone.write().await;
                        if *lock != mpaths {
                            info!("Active multipath devices changed. Previous: {:?}, New: {mpaths:?}", *lock);
                            *lock = mpaths;
                        }
                    }
                    Err(e) => {
                        error!("Error discovering active multipath devices: {e}");
                    }
                }
                tokio::time::sleep(Duration::from_secs(discovery_interval)).await;
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

        debug!("Fencer monitoring state: consecutive_failures={}, max_failures={}", fencer.consecutive_failures(), fencer.max_failures());
        let active_set = active_luns.read().await;
        debug!("Current active LUNs set: {:?}", *active_set);

        // Query multipathd
        let response = match libmultipath::send_multipath_command_to_socket(&socket, "show maps json") {
            Ok(res) => res,
            Err(e) => {
                warn!("Failed to query multipathd: {e}");
                // Incrementing consecutive failures here could trigger reboot on transient daemon restarts.
                // We just log warning as per the specification.
                continue;
            }
        };

        if fencer.update(&response, &active_set) {
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
    fn test_cli_parsing() {
        let args = vec!["pve-san-fenced", "-n", "pve01", "-t"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(cli.test_mode);
        assert_eq!(cli.node, "pve01");

        let args2 = vec!["pve-san-fenced", "-n", "pve01", "--test-mode"];
        let cli2 = Cli::try_parse_from(args2).unwrap();
        assert!(cli2.test_mode);

        let args3 = vec!["pve-san-fenced", "-n", "pve01"];
        let cli3 = Cli::try_parse_from(args3).unwrap();
        assert!(!cli3.test_mode);
        assert_eq!(cli3.sysrq_char, "b");

        let args4 = vec!["pve-san-fenced", "-n", "pve01", "--sysrq-char", "c"];
        let cli4 = Cli::try_parse_from(args4).unwrap();
        assert_eq!(cli4.sysrq_char, "c");
    }
}

