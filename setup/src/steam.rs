//! Steam's text files: library folders and per-user launch options (`localconfig.vdf`).

pub const APP_ID: &str = "4049180";
pub const GAME: &str = "Toy Story 3 Complete Edition";
/// Proton loads its own dinput8 unless told to prefer the one in the game folder.
pub const DLL_OVERRIDE: &str = r#"WINEDLLOVERRIDES="dinput8=n,b""#;
const CONFIG_VAR: &str = "TS3ONLINE_CONFIG=";

/// The `"path"` values of `libraryfolders.vdf`.
pub fn library_paths(vdf: &str) -> Vec<String> {
    vdf.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("\"path\"")?.trim();
            Some(unescape(rest.strip_prefix('"')?.strip_suffix('"')?))
        })
        .collect()
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            if let Some(n) = it.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Launch options split into words; quoted parts stay together.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        if c == '"' {
            quoted = !quoted;
            cur.push(c);
        } else if c.is_whitespace() && !quoted {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn ours(word: &str) -> bool {
    word == DLL_OVERRIDE || word.starts_with(CONFIG_VAR)
}

/// Launch options with ours in front: the dinput8 override and, when given, the config path.
pub fn with_ours(current: &str, config: Option<&str>) -> String {
    let rest: Vec<String> = words(current).into_iter().filter(|w| !ours(w)).collect();
    let mut v = vec![DLL_OVERRIDE.to_string()];
    if let Some(c) = config {
        v.push(format!("{CONFIG_VAR}\"{c}\""));
    }
    if !rest.iter().any(|w| w == "%command%") {
        v.push("%command%".into());
    }
    v.extend(rest);
    v.join(" ")
}

/// Launch options without ours; empty when only `%command%` would be left.
pub fn without_ours(current: &str) -> String {
    let rest: Vec<String> = words(current).into_iter().filter(|w| !ours(w)).collect();
    if rest.len() == 1 && rest[0] == "%command%" {
        return String::new();
    }
    rest.join(" ")
}

/// The app's launch options in a `localconfig.vdf` (inside an `apps` block); None if the app is not listed.
pub fn launch_options(vdf: &str, app: &str) -> Option<String> {
    let (start, end) = app_block(vdf, app)?;
    Some(find_value(&vdf[start..end], "LaunchOptions").map(|(_, _, v)| v).unwrap_or_default())
}

/// `vdf` with the app's launch options set to `value` (added if missing); None if the app is not
/// listed. Everything else is kept byte for byte.
pub fn set_launch_options(vdf: &str, app: &str, value: &str) -> Option<String> {
    let (start, end) = app_block(vdf, app)?;
    let block = &vdf[start..end];
    let esc = escape(value);
    let new_block = match find_value(block, "LaunchOptions") {
        Some((vs, ve, _)) => format!("{}{esc}{}", &block[..vs], &block[ve..]),
        None => {
            let indent = block.lines().find(|l| l.trim_start().starts_with('"')).map(|l| &l[..l.len() - l.trim_start().len()]).unwrap_or("\t\t\t\t\t\t");
            let body = block.trim_end_matches([' ', '\t']);
            let tail = &block[body.len()..];
            let nl = if body.ends_with('\n') { "" } else { "\n" };
            format!("{body}{nl}{indent}\"LaunchOptions\"\t\t\"{esc}\"\n{tail}")
        }
    };
    Some(format!("{}{new_block}{}", &vdf[..start], &vdf[end..]))
}

/// Byte range inside the braces of the `app` block whose parent key is `apps`.
fn app_block(vdf: &str, app: &str) -> Option<(usize, usize)> {
    let b = vdf.as_bytes();
    let mut keys: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    let mut target: Option<usize> = None;
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let (s, next) = read_string(vdf, i)?;
                pending = if pending.is_some() { None } else { Some(s) };
                i = next;
                continue;
            }
            b'{' => {
                keys.push(pending.take().unwrap_or_default());
                let n = keys.len();
                if target.is_none() && n >= 2 && keys[n - 1] == app && keys[n - 2].eq_ignore_ascii_case("apps") {
                    target = Some(n);
                    start = i + 1;
                }
            }
            b'}' => {
                if target == Some(keys.len()) {
                    return Some((start, i));
                }
                keys.pop();
                pending = None;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The quoted string at `at` (unescaped) and the index after its closing quote.
fn read_string(s: &str, at: usize) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut i = at + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some((unescape(&s[at + 1..i]), i + 1)),
            _ => i += 1,
        }
    }
    None
}

/// Byte range of the raw (escaped) value of `key` directly in `block`, and the value unescaped.
fn find_value(block: &str, key: &str) -> Option<(usize, usize, String)> {
    let b = block.as_bytes();
    let mut depth = 0;
    let mut i = 0;
    let mut pending: Option<String> = None;
    while i < b.len() {
        match b[i] {
            b'"' => {
                let (s, next) = read_string(block, i)?;
                match pending.take() {
                    Some(k) if depth == 0 && k.eq_ignore_ascii_case(key) => return Some((i + 1, next - 1, s)),
                    Some(_) => {}
                    None => pending = Some(s),
                }
                i = next;
                continue;
            }
            b'{' => {
                depth += 1;
                pending = None;
            }
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const VDF: &str = "\"UserLocalConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"10\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LaunchOptions\"\t\t\"-x\"\n\t\t\t\t\t}\n\t\t\t\t\t\"4049180\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LastPlayed\"\t\t\"1\"\n\t\t\t\t\t\t\"cloud\"\n\t\t\t\t\t\t{\n\t\t\t\t\t\t\t\"LaunchOptions\"\t\t\"nested\"\n\t\t\t\t\t\t}\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"4049180\"\t\t\"elsewhere\"\n}\n";

    #[test]
    fn libraries_are_read_and_unescaped() {
        let v = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/deck/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"G:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(library_paths(v), ["/home/deck/.local/share/Steam", "G:\\SteamLibrary"]);
    }

    #[test]
    fn launch_options_are_found_only_in_the_apps_block() {
        assert_eq!(launch_options(VDF, APP_ID).as_deref(), Some(""));
        assert_eq!(launch_options(VDF, "10").as_deref(), Some("-x"));
        assert_eq!(launch_options(VDF, "99"), None);
    }

    #[test]
    fn setting_adds_then_replaces_and_keeps_the_rest() {
        let a = set_launch_options(VDF, APP_ID, r#"A="b c" %command%"#).unwrap();
        assert_eq!(launch_options(&a, APP_ID).as_deref(), Some(r#"A="b c" %command%"#));
        assert!(a.contains("\t\t\t\t\t\t}\n\t\t\t\t\t\t\"LaunchOptions\"\t\t\"A=\\\"b c\\\" %command%\"\n\t\t\t\t\t}"));
        let b = set_launch_options(&a, APP_ID, "").unwrap();
        assert_eq!(launch_options(&b, APP_ID).as_deref(), Some(""));
        assert_eq!(launch_options(&b, "10").as_deref(), Some("-x"));
        assert!(b.starts_with("\"UserLocalConfigStore\"") && b.ends_with("\"elsewhere\"\n}\n"));
        let c = set_launch_options(VDF, "10", "-y").unwrap();
        assert_eq!(c.len(), VDF.len());
    }

    #[test]
    fn ours_go_in_front_once_and_come_out_again() {
        let cfg = "Z:/home/deck/.config/ts3-online/ts3online.ini";
        assert_eq!(with_ours("", Some(cfg)), format!(r#"{DLL_OVERRIDE} TS3ONLINE_CONFIG="{cfg}" %command%"#));
        let twice = with_ours(&with_ours("gamemoderun %command% -x", Some(cfg)), Some(cfg));
        assert_eq!(twice, format!(r#"{DLL_OVERRIDE} TS3ONLINE_CONFIG="{cfg}" gamemoderun %command% -x"#));
        assert_eq!(with_ours("-skip", None), format!("{DLL_OVERRIDE} %command% -skip"));
        assert_eq!(without_ours(&twice), "gamemoderun %command% -x");
        assert_eq!(without_ours(&with_ours("", Some(cfg))), "");
    }
}
