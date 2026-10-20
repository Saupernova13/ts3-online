//! Role and save state the save folder redirect waits on.
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Waited on by the save folder redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `net=` set, role not decided yet
    Pending,
    Host,
    Client,
    /// not configured, or failed to start
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveState {
    Waiting,
    Ready,
    Failed,
}

/// Which app data folder the game should be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Normal,
    /// client session folder holding the host save
    Session,
}

struct Shared {
    role: Role,
    save: SaveState,
    /// connect menu open: role waits do not time out
    menu_open: bool,
}

static SHARED: Mutex<Shared> = Mutex::new(Shared { role: Role::Off, save: SaveState::Waiting, menu_open: false });
static CHANGED: Condvar = Condvar::new();

/// Call before anything may wait for the role.
pub fn expect_role() {
    set_shared(|s| s.role = Role::Pending);
}

/// `wait_decision` does not count time while the menu is open.
pub fn set_menu_open(open: bool) {
    set_shared(|s| s.menu_open = open);
}

pub(super) fn set_role(role: Role) {
    set_shared(|s| s.role = role);
}

pub(super) fn set_save(state: SaveState) {
    set_shared(|s| s.save = state);
}

fn set_shared(f: impl FnOnce(&mut Shared)) {
    if let Ok(mut g) = SHARED.lock() {
        f(&mut g);
        CHANGED.notify_all();
    }
}

/// Waits for the role and a client's host save; `timeout` runs only while the connect menu is closed.
pub fn wait_decision(timeout: Duration) -> Decision {
    let mut deadline: Option<Instant> = None;
    let Ok(mut g) = SHARED.lock() else { return Decision::Normal };
    loop {
        match (g.role, g.save) {
            (Role::Off | Role::Host, _) | (Role::Client, SaveState::Failed) => return Decision::Normal,
            (Role::Client, SaveState::Ready) => return Decision::Session,
            _ => {}
        }
        if g.menu_open {
            deadline = None;
            g = match CHANGED.wait(g) {
                Ok(g) => g,
                Err(_) => return Decision::Normal,
            };
            continue;
        }
        let end = *deadline.get_or_insert_with(|| Instant::now() + timeout);
        let Some(left) = end.checked_duration_since(Instant::now()) else { return Decision::Normal };
        g = match CHANGED.wait_timeout(g, left) {
            Ok((g, _)) => g,
            Err(_) => return Decision::Normal,
        };
    }
}

pub fn describe() -> String {
    SHARED.lock().map(|g| format!("role {:?}, save {:?}", g.role, g.save)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_waits_for_role_and_save() {
        // one test owns the shared state, so the cases run in order
        set_shared(|s| {
            s.role = Role::Off;
            s.save = SaveState::Waiting;
            s.menu_open = false;
        });
        assert_eq!(wait_decision(Duration::from_secs(5)), Decision::Normal); // off: no wait
        set_role(Role::Pending);
        let t0 = Instant::now();
        assert_eq!(wait_decision(Duration::from_millis(150)), Decision::Normal); // times out
        assert!(t0.elapsed() >= Duration::from_millis(140));
        let h = std::thread::spawn(|| wait_decision(Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(50));
        set_role(Role::Client);
        std::thread::sleep(Duration::from_millis(50));
        set_save(SaveState::Ready);
        assert_eq!(h.join().unwrap_or(Decision::Normal), Decision::Session);
        set_save(SaveState::Failed);
        assert_eq!(wait_decision(Duration::from_secs(5)), Decision::Normal);
        set_role(Role::Host);
        set_save(SaveState::Ready);
        assert_eq!(wait_decision(Duration::from_secs(5)), Decision::Normal); // a host keeps its folder
        // an open menu stops the clock
        set_role(Role::Pending);
        set_save(SaveState::Waiting);
        set_menu_open(true);
        let h = std::thread::spawn(|| wait_decision(Duration::from_millis(100)));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!h.is_finished(), "waited out its timeout while the menu was open");
        set_role(Role::Client);
        set_save(SaveState::Ready);
        set_menu_open(false);
        assert_eq!(h.join().unwrap_or(Decision::Normal), Decision::Session);
        // closing the menu starts the timeout
        set_role(Role::Pending);
        set_menu_open(true);
        let h = std::thread::spawn(|| wait_decision(Duration::from_millis(150)));
        std::thread::sleep(Duration::from_millis(300));
        let t0 = Instant::now();
        set_menu_open(false);
        assert_eq!(h.join().unwrap_or(Decision::Session), Decision::Normal);
        assert!(t0.elapsed() >= Duration::from_millis(140));
        set_role(Role::Off);
        set_save(SaveState::Waiting);
    }
}
