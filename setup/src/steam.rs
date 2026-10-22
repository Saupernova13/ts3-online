//! Steam's library list (`libraryfolders.vdf`), to find the game.

pub const APP_ID: &str = "4049180";
pub const GAME: &str = "Toy Story 3 Complete Edition";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libraries_are_read_and_unescaped() {
        let v = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/deck/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"G:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(library_paths(v), ["/home/deck/.local/share/Steam", "G:\\SteamLibrary"]);
    }
}
