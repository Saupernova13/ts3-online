//! The setup as questions in a terminal: for a system where the window cannot open (no desktop, or
//! no graphics libraries), or with `--text`.
use crate::actions;
use crate::platform;
use crate::settings::{Role, Settings};
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn ask(question: &str, current: &str) -> String {
    if current.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{current}]: ");
    }
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
        return current.to_string();
    }
    let t = line.trim();
    if t.is_empty() {
        current.to_string()
    } else {
        t.to_string()
    }
}

/// Runs the questions; the process exit code.
pub fn run(user: &str) -> i32 {
    println!("ts3-online setup {}: online co-op for Toy Story 3\n", env!("CARGO_PKG_VERSION"));
    let mut game = platform::find_game();
    while game.is_none() {
        let p = ask("Toy Story 3's game folder (Steam: right-click the game, Manage, Browse local files)", "");
        if p.is_empty() {
            return 1;
        }
        game = Some(PathBuf::from(p)).filter(|g| actions::game_dir_ok(g));
        if game.is_none() {
            println!("No {} there.", platform::EXE);
        }
    }
    let Some(game) = game else { return 1 };
    println!("Game folder: {}", game.display());
    let installed = actions::installed(&game);
    let text = actions::read_config(Some(&game));
    let mut s = if text.trim().is_empty() { Settings::new(user) } else { Settings::from_ini(&text, user) };
    let current = match s.role {
        Role::Host => "1",
        Role::Join => "2",
        Role::Ask => "3",
    };
    println!("\nHow do you want to play?\n  1  Host a game: friends join you, every time the game starts\n  2  Join a friend: your game joins theirs every time it starts\n  3  Choose each time: a window opens when the game starts");
    s.role = match ask("1, 2 or 3", current).as_str() {
        "1" => Role::Host,
        "3" => Role::Ask,
        _ => Role::Join,
    };
    s.name = ask("Your name", &s.name);
    s.password = ask("Password (optional; everyone uses the same; - for none)", &s.password);
    if s.password == "-" {
        s.password.clear();
    }
    if s.role == Role::Join {
        if !s.recent.is_empty() {
            println!("Joined before: {}", s.recent.join(", "));
        }
        s.join = ask("The host's address (their Tailscale or network address)", &s.join);
    }
    if let Some(p) = s.problem() {
        println!("{p}");
        return 2;
    }
    let steps = if installed.is_none() { actions::install_all(&game, &s) } else { actions::apply(&game, &s, actions::bundled_release(&game).as_ref()) };
    for st in &steps {
        println!("{} {}", if st.ok { "ok  " } else { "FAIL" }, st.text);
    }
    if steps.iter().all(|st| st.ok) {
        println!("\nDone. Start Toy Story 3 from Steam. Run this setup again to change anything.");
        0
    } else {
        1
    }
}
