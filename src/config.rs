use regex::Regex;

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(String),
    OpenBrace,
    CloseBrace,
}

pub fn tokenize(config: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = config.char_indices().peekable();
    let mut current_word = String::new();
    let mut in_quote = false;

    while let Some((_, c)) = chars.next() {
        if in_quote {
            if c == '"' {
                if chars.peek().map(|&(_, next_c)| next_c) == Some('"') {
                    chars.next();
                    current_word.push('"');
                } else {
                    in_quote = false;
                    tokens.push(Token::Word(current_word.clone()));
                    current_word.clear();
                }
            } else {
                current_word.push(c);
            }
        } else {
            match c {
                c if c.is_whitespace() => {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                }
                '#' | '!' => {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                    while let Some(&(_, next_c)) = chars.peek() {
                        if next_c == '\n' || next_c == '\r' {
                            break;
                        }
                        chars.next();
                    }
                }
                '{' => {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                    tokens.push(Token::OpenBrace);
                }
                '}' => {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                    tokens.push(Token::CloseBrace);
                }
                '"' => {
                    if !current_word.is_empty() {
                        tokens.push(Token::Word(current_word.clone()));
                        current_word.clear();
                    }
                    in_quote = true;
                }
                _ => {
                    current_word.push(c);
                }
            }
        }
    }

    if !current_word.is_empty() {
        tokens.push(Token::Word(current_word));
    }

    tokens
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeviceConfig {
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub dev_loss_tmo: Option<String>,
    pub no_path_retry: Option<String>,
    pub polling_interval: Option<u64>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MultipathConfig {
    pub defaults: DeviceConfig,
    pub devices: Vec<DeviceConfig>,
    pub overrides: DeviceConfig,
}

pub fn parse_multipath_config(config_str: &str) -> MultipathConfig {
    let tokens = tokenize(config_str);
    let mut config = MultipathConfig::default();
    let mut iter = tokens.into_iter().peekable();

    while let Some(token) = iter.next() {
        match token {
            Token::Word(w) if w == "defaults" => {
                if let Some(Token::OpenBrace) = iter.next() {
                    config.defaults = parse_block(&mut iter);
                }
            }
            Token::Word(w) if w == "devices" => {
                if let Some(Token::OpenBrace) = iter.next() {
                    while let Some(t) = iter.next() {
                        match t {
                            Token::Word(w) if w == "device" => {
                                if let Some(Token::OpenBrace) = iter.next() {
                                    config.devices.push(parse_block(&mut iter));
                                }
                            }
                            Token::CloseBrace => break,
                            Token::Word(_) | Token::OpenBrace => {}
                        }
                    }
                }
            }
            Token::Word(w) if w == "overrides" => {
                if let Some(Token::OpenBrace) = iter.next() {
                    config.overrides = parse_block(&mut iter);
                }
            }
            Token::Word(_) => {
                // skip blocks we don't care about
                if let Some(Token::OpenBrace) = iter.next() {
                    let mut depth = 1;
                    while depth > 0 {
                        match iter.next() {
                            Some(Token::OpenBrace) => depth += 1,
                            Some(Token::CloseBrace) => depth -= 1,
                            None => break,
                            Some(Token::Word(_)) => {}
                        }
                    }
                }
            }
            Token::OpenBrace | Token::CloseBrace => {}
        }
    }
    config
}

fn is_known_key(s: &str) -> bool {
    matches!(
        s,
        "vendor"
            | "product"
            | "dev_loss_tmo"
            | "no_path_retry"
            | "defaults"
            | "devices"
            | "device"
            | "overrides"
            | "path_grouping_policy"
            | "path_selector"
            | "path_checker"
            | "features"
            | "prio"
            | "failback"
            | "rr_weight"
            | "rr_min_io"
            | "rr_min_io_rq"
            | "fast_io_fail_tmo"
            | "polling_interval"
    )
}

fn parse_block(iter: &mut std::iter::Peekable<impl Iterator<Item = Token>>) -> DeviceConfig {
    let mut block = DeviceConfig::default();
    let mut depth = 1;
    while let Some(token) = iter.next() {
        match token {
            Token::OpenBrace => depth += 1,
            Token::CloseBrace => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Token::Word(key) if depth == 1 => {
                let has_val = if let Some(next_token) = iter.peek() {
                    match next_token {
                        Token::Word(val) => {
                            if is_known_key(val) {
                                warn!(
                                    "Key '{}' has no value, next token is a known key '{}'",
                                    key, val
                                );
                                false
                            } else {
                                true
                            }
                        }
                        Token::OpenBrace | Token::CloseBrace => {
                            warn!("Key '{}' has no value, next token is brace", key);
                            false
                        }
                    }
                } else {
                    warn!("Key '{}' has no value at end of config", key);
                    false
                };

                if has_val {
                    if let Some(Token::Word(val)) = iter.next() {
                        match key.as_str() {
                            "vendor" => block.vendor = Some(val),
                            "product" => block.product = Some(val),
                            "dev_loss_tmo" => block.dev_loss_tmo = Some(val),
                            "no_path_retry" => block.no_path_retry = Some(val),
                            "polling_interval" => block.polling_interval = val.parse::<u64>().ok(),
                            _ => {}
                        }
                    }
                }
            }
            Token::Word(_) => {}
        }
    }
    block
}

pub fn get_merged_config(config: &MultipathConfig, vendor: &str, product: &str) -> DeviceConfig {
    let mut merged = config.defaults.clone();

    for device in &config.devices {
        let vendor_match = match &device.vendor {
            Some(v) => match Regex::new(v) {
                Ok(re) => re.is_match(vendor),
                Err(e) => {
                    warn!("Invalid vendor regex '{}' in devices config: {}", v, e);
                    false
                }
            },
            None => true, // If not specified, matches any
        };
        let product_match = match &device.product {
            Some(p) => match Regex::new(p) {
                Ok(re) => re.is_match(product),
                Err(e) => {
                    warn!("Invalid product regex '{}' in devices config: {}", p, e);
                    false
                }
            },
            None => true,
        };

        if vendor_match && product_match {
            if device.dev_loss_tmo.is_some() {
                merged.dev_loss_tmo = device.dev_loss_tmo.clone();
            }
            if device.no_path_retry.is_some() {
                merged.no_path_retry = device.no_path_retry.clone();
            }
            if device.polling_interval.is_some() {
                merged.polling_interval = device.polling_interval;
            }
        }
    }

    if config.overrides.dev_loss_tmo.is_some() {
        merged.dev_loss_tmo = config.overrides.dev_loss_tmo.clone();
    }
    if config.overrides.no_path_retry.is_some() {
        merged.no_path_retry = config.overrides.no_path_retry.clone();
    }
    if config.overrides.polling_interval.is_some() {
        merged.polling_interval = config.overrides.polling_interval;
    }

    merged
}

/// Compiled vendor or product pattern for a device entry.
///
/// `Absent` means no pattern was specified in the config (matches any vendor/product).
/// `Invalid` means the pattern failed to compile (matches nothing, same as the
/// previous inline behavior where invalid regexes were logged and treated as
/// non-matching). `Compiled` holds the ready-to-use regex.
enum CompiledPattern {
    Absent,
    Invalid,
    Compiled(Regex),
}

/// A device entry with pre-compiled vendor and product regex patterns.
///
/// Built from a parsed [`MultipathConfig`] to avoid recompiling the same
/// regexes on every poll cycle. See [`compile_devices`].
struct CompiledDevice {
    vendor: CompiledPattern,
    product: CompiledPattern,
    config: DeviceConfig,
}

/// Pre-compiles vendor and product regex patterns from all device entries
/// in a parsed [`MultipathConfig`]. Invalid regexes are logged and treated
/// as non-matching (same behavior as the previous inline compilation).
fn compile_devices(config: &MultipathConfig) -> Vec<CompiledDevice> {
    config
        .devices
        .iter()
        .map(|device| {
            let vendor = match &device.vendor {
                None => CompiledPattern::Absent,
                Some(v) => match Regex::new(v) {
                    Ok(re) => CompiledPattern::Compiled(re),
                    Err(e) => {
                        warn!("Invalid vendor regex '{}' in devices config: {}", v, e);
                        CompiledPattern::Invalid
                    }
                },
            };
            let product = match &device.product {
                None => CompiledPattern::Absent,
                Some(p) => match Regex::new(p) {
                    Ok(re) => CompiledPattern::Compiled(re),
                    Err(e) => {
                        warn!("Invalid product regex '{}' in devices config: {}", p, e);
                        CompiledPattern::Invalid
                    }
                },
            };
            CompiledDevice {
                vendor,
                product,
                config: device.clone(),
            }
        })
        .collect()
}

/// Returns the merged [`DeviceConfig`] for a given vendor/product by matching
/// against pre-compiled device regex patterns. Absent patterns match anything;
/// invalid patterns match nothing (same semantics as `get_merged_config`).
fn get_merged_config_compiled(
    defaults: &DeviceConfig,
    compiled_devices: &[CompiledDevice],
    overrides: &DeviceConfig,
    vendor: &str,
    product: &str,
) -> DeviceConfig {
    let mut merged = defaults.clone();

    for device in compiled_devices {
        let vendor_match = match &device.vendor {
            CompiledPattern::Absent => true,
            CompiledPattern::Invalid => false,
            CompiledPattern::Compiled(re) => re.is_match(vendor),
        };
        let product_match = match &device.product {
            CompiledPattern::Absent => true,
            CompiledPattern::Invalid => false,
            CompiledPattern::Compiled(re) => re.is_match(product),
        };

        if vendor_match && product_match {
            if device.config.dev_loss_tmo.is_some() {
                merged.dev_loss_tmo = device.config.dev_loss_tmo.clone();
            }
            if device.config.no_path_retry.is_some() {
                merged.no_path_retry = device.config.no_path_retry.clone();
            }
            if device.config.polling_interval.is_some() {
                merged.polling_interval = device.config.polling_interval;
            }
        }
    }

    if overrides.dev_loss_tmo.is_some() {
        merged.dev_loss_tmo = overrides.dev_loss_tmo.clone();
    }
    if overrides.no_path_retry.is_some() {
        merged.no_path_retry = overrides.no_path_retry.clone();
    }
    if overrides.polling_interval.is_some() {
        merged.polling_interval = overrides.polling_interval;
    }

    merged
}

use crate::MultipathMap;
use log::warn;
use std::collections::HashSet;

pub fn validate_merged_device_config(merged: &DeviceConfig, fencing_time_sec: u64) -> Vec<String> {
    let mut map_warnings = Vec::new();

    let polling_interval = merged.polling_interval.unwrap_or(5);
    let min_queue_time = fencing_time_sec + 15;
    let mut queue_time_sec: Option<u64> = None;

    match merged.no_path_retry.as_deref() {
        Some("queue") => {}
        Some(val) => {
            if let Ok(num) = val.parse::<u64>() {
                if num == 0 {
                    map_warnings.push("no_path_retry is set to '0' (queueing disabled). Expected 'queue' or a safe high numeric value".to_string());
                } else {
                    let qt = num * polling_interval;
                    queue_time_sec = Some(qt);
                    if qt < min_queue_time {
                        map_warnings.push(format!(
                            "no_path_retry is set to '{val}', yielding a queue time of {qt}s (with {polling_interval}s polling_interval). This is too low (expected >= {min_queue_time}s to allow fencing)"
                        ));
                    }
                }
            } else if val == "fail" {
                map_warnings.push("no_path_retry is set to 'fail' (queueing disabled). Expected 'queue' or a safe high numeric value".to_string());
            } else {
                map_warnings.push(format!(
                    "no_path_retry is set to '{val}' instead of 'queue' or a safe high numeric value"
                ));
            }
        }
        None => map_warnings.push(
            "no_path_retry is not configured (expected 'queue' or a safe high numeric value)"
                .to_string(),
        ),
    }

    let min_dev_loss_tmo = fencing_time_sec + 60;
    match merged.dev_loss_tmo.as_deref() {
        Some("infinity") => {}
        Some(val) => {
            if let Ok(num) = val.parse::<u64>() {
                if num < min_dev_loss_tmo {
                    map_warnings.push(format!(
                        "dev_loss_tmo is set to '{val}' which is too low (expected 'infinity' or >= {min_dev_loss_tmo})"
                    ));
                }
                if let Some(qt) = queue_time_sec {
                    if num <= qt {
                        map_warnings.push(format!(
                            "dev_loss_tmo ({num}s) must be strictly greater than no_path_retry queue time ({qt}s) to avoid deadlocks"
                        ));
                    }
                }
            } else {
                map_warnings.push(format!(
                    "dev_loss_tmo is set to '{val}' instead of 'infinity' or a safe high number"
                ));
            }
        }
        None => map_warnings.push(
            "dev_loss_tmo is not configured (expected 'infinity' or a safe high number)"
                .to_string(),
        ),
    }

    map_warnings
}

pub fn check_maps_config(
    maps: &[MultipathMap],
    active_luns: &HashSet<String>,
    config_str: &str,
    fencing_time_sec: u64,
) {
    let parsed_config = parse_multipath_config(config_str);
    let compiled_devices = compile_devices(&parsed_config);
    let mut all_warnings = Vec::new();

    let monitored_maps: Vec<&MultipathMap> = maps
        .iter()
        .filter(|map| {
            active_luns.iter().any(|lun| {
                if lun.contains('+') {
                    let parts: Vec<&str> = lun.split('+').collect();
                    parts.contains(&map.name.as_str()) || parts.contains(&map.uuid.as_str())
                } else {
                    lun == &map.name || lun == &map.uuid
                }
            })
        })
        .collect();

    for map in monitored_maps {
        let vendor = map.vend.as_deref().unwrap_or("");
        let product = map.prod.as_deref().unwrap_or("");

        let merged = get_merged_config_compiled(
            &parsed_config.defaults,
            &compiled_devices,
            &parsed_config.overrides,
            vendor,
            product,
        );

        let map_warnings = validate_merged_device_config(&merged, fencing_time_sec);
        if !map_warnings.is_empty() {
            all_warnings.push(format!(
                "Map {} (vendor: {}, product: {}): {}",
                map.name,
                vendor,
                product,
                map_warnings.join(", ")
            ));
        }
    }

    for warning in &all_warnings {
        warn!(
            "Multipath configuration recommendation warning: {}",
            warning
        );
    }

    if !all_warnings.is_empty() {
        let msg = format!(
            "Multipath configuration recommendation warnings: {}",
            all_warnings.join("; ")
        );
        crate::status::get_status_tracker().set_issue(
            "config_warnings",
            crate::status::StatusLevel::Warning,
            msg,
        );
    } else {
        crate::status::get_status_tracker().clear_issue("config_warnings");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_multipath_config_defaults() {
        let config_str = r#"
defaults {
    polling_interval 5
    no_path_retry "queue"
    fast_io_fail_tmo 5
    dev_loss_tmo "infinity"
}
"#;
        let config = parse_multipath_config(config_str);
        assert_eq!(config.defaults.dev_loss_tmo.as_deref(), Some("infinity"));
        assert_eq!(config.defaults.no_path_retry.as_deref(), Some("queue"));
    }

    #[test]
    fn test_parse_multipath_config_bug1_and_bug3() {
        // Bug 1: nested block at depth 1, e.g. device with nested some_block.
        // If some_block causes premature exits, product won't be parsed.
        let config_str = r#"
devices {
    device {
        vendor "HUAWEI"
        some_block {
            foo bar
        }
        product "XSG1"
        dev_loss_tmo 30
    }
}
"#;
        let config = parse_multipath_config(config_str);
        assert_eq!(config.devices.len(), 1);
        assert_eq!(config.devices[0].vendor.as_deref(), Some("HUAWEI"));
        assert_eq!(config.devices[0].product.as_deref(), Some("XSG1"));
        assert_eq!(config.devices[0].dev_loss_tmo.as_deref(), Some("30"));

        // Bug 3: key with missing value followed by another key.
        let config_str_bug3 = r#"
defaults {
    vendor
    dev_loss_tmo "infinity"
    no_path_retry "queue"
}
"#;
        let config_bug3 = parse_multipath_config(config_str_bug3);
        assert_eq!(config_bug3.defaults.vendor, None);
        assert_eq!(
            config_bug3.defaults.dev_loss_tmo.as_deref(),
            Some("infinity")
        );
        assert_eq!(config_bug3.defaults.no_path_retry.as_deref(), Some("queue"));
    }

    #[test]
    fn test_regex_compilation_warning() {
        let config_str = r#"
devices {
    device {
        vendor "["
        product "XSG1"
        dev_loss_tmo 30
    }
}
"#;
        let parsed = parse_multipath_config(config_str);
        let merged = get_merged_config(&parsed, "HUAWEI", "XSG1");
        // Should not crash, and should fallback (vendor match fails because Regex "[ " is invalid)
        assert_ne!(merged.dev_loss_tmo.as_deref(), Some("30"));
    }

    #[test]
    fn test_parse_multipath_config_overrides() {
        let config_str = r#"
defaults {
    no_path_retry "queue"
    dev_loss_tmo "120"
}
devices {
    device {
        vendor "HUAWEI"
        product "XSG1"
        dev_loss_tmo 30
    }
}
overrides {
    dev_loss_tmo "infinity"
}
"#;
        let config = parse_multipath_config(config_str);

        // Verify the entire defaults object
        let expected_defaults = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("120".to_string()),
            no_path_retry: Some("queue".to_string()),
            polling_interval: None,
        };
        assert_eq!(config.defaults, expected_defaults);

        // Verify the parsed overrides
        let expected_overrides = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("infinity".to_string()),
            no_path_retry: None,
            polling_interval: None,
        };
        assert_eq!(config.overrides, expected_overrides);

        // Verify the merging logic with overrides prioritizing over devices
        let merged = get_merged_config(&config, "HUAWEI", "XSG1");
        let expected_merged = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("infinity".to_string()),
            no_path_retry: Some("queue".to_string()),
            polling_interval: None,
        };
        assert_eq!(merged, expected_merged);
    }

    #[test]
    fn test_parse_multipath_config_numeric_no_path_retry() {
        let config_str = r#"
defaults {
    no_path_retry 12
    dev_loss_tmo 120
}
"#;
        let config = parse_multipath_config(config_str);

        let expected_defaults = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("120".to_string()),
            no_path_retry: Some("12".to_string()),
            polling_interval: None,
        };
        assert_eq!(config.defaults, expected_defaults);
    }

    #[test]
    fn test_parse_multipath_config_numeric_polling_interval() {
        let config_str = r#"
defaults {
    no_path_retry 12
    polling_interval 10
}
"#;
        let config = parse_multipath_config(config_str);

        let expected_defaults = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: None,
            no_path_retry: Some("12".to_string()),
            polling_interval: Some(10),
        };
        assert_eq!(config.defaults, expected_defaults);
    }

    #[test]
    fn test_validate_merged_device_config() {
        let fencing_time_sec = 30; // min_queue_time = 45, min_dev_loss_tmo = 90

        // Test: Valid numeric no_path_retry and valid infinity dev_loss_tmo
        let mut config = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("infinity".to_string()),
            no_path_retry: Some("12".to_string()), // 12 * 5 = 60s (>= 45s)
            polling_interval: Some(5),
        };
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings.is_empty());

        // Test: Valid numeric dev_loss_tmo strictly greater than queue time
        config.dev_loss_tmo = Some("90".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings.is_empty());

        // Test: Invalid no_path_retry = 0
        config.no_path_retry = Some("0".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings.iter().any(|w| w.contains("queueing disabled")));

        // Test: Invalid no_path_retry = fail
        config.no_path_retry = Some("fail".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings.iter().any(|w| w.contains("queueing disabled")));

        // Test: Invalid no_path_retry string
        config.no_path_retry = Some("invalid".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings
            .iter()
            .any(|w| w.contains("instead of 'queue' or a safe high numeric value")));

        // Test: Valid queue, invalid dev_loss_tmo string
        config.no_path_retry = Some("queue".to_string());
        config.dev_loss_tmo = Some("invalid".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings
            .iter()
            .any(|w| w.contains("instead of 'infinity' or a safe high number")));

        // Test: Numeric dev_loss_tmo too low (< 90)
        config.dev_loss_tmo = Some("80".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings.iter().any(|w| w.contains("which is too low")));

        // Test: Queue time too low (< 45)
        config.dev_loss_tmo = Some("infinity".to_string());
        config.no_path_retry = Some("8".to_string()); // 8 * 5 = 40s
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings
            .iter()
            .any(|w| w.contains("yielding a queue time of 40s")));

        // Test: dev_loss_tmo not strictly greater than queue time
        config.no_path_retry = Some("20".to_string()); // 20 * 5 = 100s
        config.dev_loss_tmo = Some("100".to_string()); // 100 <= 100
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings
            .iter()
            .any(|w| w.contains("must be strictly greater than")));

        // Test: missing dev_loss_tmo
        config.dev_loss_tmo = None;
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings
            .iter()
            .any(|w| w.contains("dev_loss_tmo is not configured")));

        // Test: missing no_path_retry
        config.no_path_retry = None;
        config.dev_loss_tmo = Some("infinity".to_string());
        let warnings = validate_merged_device_config(&config, fencing_time_sec);
        assert!(warnings
            .iter()
            .any(|w| w.contains("no_path_retry is not configured")));
    }

    #[test]
    fn test_compile_devices_and_get_merged_config_compiled() {
        let config_str = r#"
defaults {
    no_path_retry "queue"
    dev_loss_tmo "120"
}
devices {
    device {
        vendor "HUAWEI"
        product "XSG1"
        dev_loss_tmo 30
    }
    device {
        vendor "DELL"
        product "ME4"
        no_path_retry 20
    }
}
overrides {
    dev_loss_tmo "infinity"
}
"#;
        let config = parse_multipath_config(config_str);
        let compiled = compile_devices(&config);
        assert_eq!(compiled.len(), 2);

        // First device: vendor and product regex compiled successfully
        assert!(matches!(compiled[0].vendor, CompiledPattern::Compiled(_)));
        assert!(matches!(compiled[0].product, CompiledPattern::Compiled(_)));

        // Second device: vendor and product regex compiled successfully
        assert!(matches!(compiled[1].vendor, CompiledPattern::Compiled(_)));
        assert!(matches!(compiled[1].product, CompiledPattern::Compiled(_)));

        // Matching device: HUAWEI/XSG1 should get dev_loss_tmo 30 from device,
        // overridden to "infinity" by overrides section
        let merged = get_merged_config_compiled(
            &config.defaults,
            &compiled,
            &config.overrides,
            "HUAWEI",
            "XSG1",
        );
        let expected = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("infinity".to_string()),
            no_path_retry: Some("queue".to_string()),
            polling_interval: None,
        };
        assert_eq!(merged, expected);

        // Matching second device: DELL/ME4 gets no_path_retry 20 from device,
        // dev_loss_tmo "infinity" from overrides
        let merged_dell = get_merged_config_compiled(
            &config.defaults,
            &compiled,
            &config.overrides,
            "DELL",
            "ME4",
        );
        assert_eq!(merged_dell.no_path_retry.as_deref(), Some("20"));
        assert_eq!(merged_dell.dev_loss_tmo.as_deref(), Some("infinity"));

        // Non-matching vendor/product: should get defaults + overrides only
        let merged_unknown = get_merged_config_compiled(
            &config.defaults,
            &compiled,
            &config.overrides,
            "UNKNOWN",
            "UNKNOWN",
        );
        assert_eq!(merged_unknown.no_path_retry.as_deref(), Some("queue"));
        assert_eq!(merged_unknown.dev_loss_tmo.as_deref(), Some("infinity"));
    }

    #[test]
    fn test_compile_devices_invalid_regex() {
        let config_str = r#"
devices {
    device {
        vendor "["
        product "XSG1"
        dev_loss_tmo 30
    }
}
"#;
        let config = parse_multipath_config(config_str);
        let compiled = compile_devices(&config);
        assert_eq!(compiled.len(), 1);
        // Invalid regex should compile to Invalid (non-matching)
        assert!(matches!(compiled[0].vendor, CompiledPattern::Invalid));
        assert!(matches!(compiled[0].product, CompiledPattern::Compiled(_)));

        // Vendor with invalid regex should not match, so dev_loss_tmo from
        // device should NOT be applied
        let merged = get_merged_config_compiled(
            &config.defaults,
            &compiled,
            &config.overrides,
            "HUAWEI",
            "XSG1",
        );
        assert_ne!(merged.dev_loss_tmo.as_deref(), Some("30"));
    }

    #[test]
    fn test_compile_devices_wildcard_patterns() {
        let config_str = r#"
devices {
    device {
        vendor ".*"
        product ".*"
        dev_loss_tmo 60
    }
}
"#;
        let config = parse_multipath_config(config_str);
        let compiled = compile_devices(&config);
        assert_eq!(compiled.len(), 1);
        // Wildcard patterns should compile successfully
        assert!(matches!(compiled[0].vendor, CompiledPattern::Compiled(_)));
        assert!(matches!(compiled[0].product, CompiledPattern::Compiled(_)));

        // Should match any vendor/product
        let merged = get_merged_config_compiled(
            &config.defaults,
            &compiled,
            &config.overrides,
            "ANYTHING",
            "ANYTHING",
        );
        assert_eq!(merged.dev_loss_tmo.as_deref(), Some("60"));
    }
}
