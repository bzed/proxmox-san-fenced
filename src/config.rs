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
                                warn!("Key '{}' has no value, next token is a known key '{}'", key, val);
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
        }
    }

    if config.overrides.dev_loss_tmo.is_some() {
        merged.dev_loss_tmo = config.overrides.dev_loss_tmo.clone();
    }
    if config.overrides.no_path_retry.is_some() {
        merged.no_path_retry = config.overrides.no_path_retry.clone();
    }

    merged
}

use crate::MultipathMap;
use log::warn;
use std::collections::HashSet;

pub fn check_maps_config(maps: &[MultipathMap], active_luns: &HashSet<String>, config_str: &str) {
    let parsed_config = parse_multipath_config(config_str);
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

        let merged = get_merged_config(&parsed_config, vendor, product);

        let mut map_warnings = Vec::new();

        match merged.dev_loss_tmo.as_deref() {
            Some("infinity") => {}
            Some(val) => map_warnings.push(format!(
                "dev_loss_tmo is set to '{val}' instead of 'infinity'"
            )),
            None => {
                map_warnings.push("dev_loss_tmo is not configured (expected infinity)".to_string())
            }
        }

        match merged.no_path_retry.as_deref() {
            Some("queue") => {}
            Some(val) => map_warnings.push(format!(
                "no_path_retry is set to '{val}' instead of 'queue'"
            )),
            None => {
                map_warnings.push("no_path_retry is not configured (expected queue)".to_string())
            }
        }

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
        assert_eq!(config_bug3.defaults.dev_loss_tmo.as_deref(), Some("infinity"));
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
        };
        assert_eq!(config.defaults, expected_defaults);

        // Verify the parsed overrides
        let expected_overrides = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("infinity".to_string()),
            no_path_retry: None,
        };
        assert_eq!(config.overrides, expected_overrides);

        // Verify the merging logic with overrides prioritizing over devices
        let merged = get_merged_config(&config, "HUAWEI", "XSG1");
        let expected_merged = DeviceConfig {
            vendor: None,
            product: None,
            dev_loss_tmo: Some("infinity".to_string()),
            no_path_retry: Some("queue".to_string()),
        };
        assert_eq!(merged, expected_merged);
    }
}
