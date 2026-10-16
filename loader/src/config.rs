//! `ts3online.ini`: optional `key=value` lines, `#` comments.

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub enabled: bool,
    pub watch: bool,
    pub watch_interval_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: true, watch: true, watch_interval_ms: 500 }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Unknown keys and unparsable values are ignored (the default stays).
pub fn parse(text: &str) -> Config {
    let mut c = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match k.as_str() {
            "enabled" => c.enabled = parse_bool(v).unwrap_or(c.enabled),
            "watch" => c.watch = parse_bool(v).unwrap_or(c.watch),
            "watch_interval_ms" => {
                if let Ok(n) = v.parse::<u32>() {
                    c.watch_interval_ms = n.max(10);
                }
            }
            _ => {}
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(parse(""), Config { enabled: true, watch: true, watch_interval_ms: 500 });
    }

    #[test]
    fn overrides_and_comments() {
        let c = parse("# hi\n enabled = 0 \nwatch=false\nWATCH_INTERVAL_MS=250\nbogus=1\nnoequals\n");
        assert_eq!(c, Config { enabled: false, watch: false, watch_interval_ms: 250 });
    }

    #[test]
    fn bad_values_keep_defaults() {
        let c = parse("enabled=maybe\nwatch_interval_ms=abc\n");
        assert_eq!(c, Config::default());
        assert_eq!(parse("watch_interval_ms=0").watch_interval_ms, 10);
    }
}
