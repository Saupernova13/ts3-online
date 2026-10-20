//! The connect menu (`net=menu`): a small Win32 window on its own thread that lists the games found
//! on the tailnet and LAN and lets the player join one or host. The choice starts the net session.
//! The logic lives in `menu_model`; this file is the window, the discovery socket and the glue.
use crate::config::{Net, NetKey};
use crate::menu_model::{self as model, Choice, Table, Verdict};
use crate::net::proto::{self, DecodeError, Msg};
use crate::net::{self, discover};
use crate::win::wide;
use crate::log;
use std::cell::{Cell, RefCell};
use std::ffi::{c_void, OsStr};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateFontW, DeleteObject, GetStockObject, DEFAULT_GUI_FONT, HBRUSH, HGDIOBJ};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BringWindowToTop, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetWindow, GW_CHILD, GW_HWNDNEXT,
    GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW, IsDialogMessageW, KillTimer, LoadCursorW, PostQuitMessage,
    RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    UnregisterClassW, BN_CLICKED, BS_PUSHBUTTON, ES_AUTOHSCROLL, ES_PASSWORD, HMENU,
    HWND_TOPMOST, IDC_ARROW, LBN_DBLCLK, LBN_SELCHANGE, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LB_ADDSTRING, LB_ERR,
    LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, MSG, SM_CXSCREEN, SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE,
    SW_SHOW, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_SETFONT, WM_TIMER, WNDCLASSW, WS_BORDER,
    WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_TOPMOST, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

const TITLE: &str = concat!("Toy Story 3 Online ", env!("CARGO_PKG_VERSION"));
pub const CLASS: &str = "Ts3OnlineConnectMenu";
const TIMER_ID: usize = 1;
const TIMER_MS: u32 = 250;
const PROBE_EVERY_MS: u64 = 1000;
const TAILNET_EVERY_MS: u64 = 15_000;
const WM_ENTER: u32 = WM_APP + 1;
const VK_RETURN: usize = 0x0D;
const COLOR_BTNFACE: u32 = 15;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const SS_LEFT: i32 = 0;
const ID_NAME: i32 = 101;
const ID_KEY: i32 = 102;
const ID_LIST: i32 = 103;
const ID_ADDR: i32 = 104;
const ID_JOIN: i32 = 105;
const ID_HOST: i32 = 106;
const CLIENT_W: i32 = 400;
const CLIENT_H: i32 = 400;
/// Where the "This PC" line looks for the LAN address (nothing is sent there).
const ROUTE_PROBE: ([u8; 4], u16) = ([8, 8, 8, 8], 53);

/// What `menu::start` needs from `attach`.
pub struct Init {
    /// the ini the config came from, to save the choices into
    pub ini: Option<PathBuf>,
    pub opts: net::Options,
    /// `menu_last` and the config's own values, to see what changed
    pub last: String,
}

struct Picked {
    choice: Choice,
    name: String,
    key: String,
}

/// Opens the menu on its own thread and returns at once. The net session starts when the player chooses.
pub fn start(init: Init) {
    let headers = crate::win::ProcMem.read_headers(crate::win::main_module_base());
    let Some(build) = net::build_id(&headers) else {
        log!("menu: cannot read the PE headers; skipping the menu");
        return net::start(&Net::Auto, init.opts);
    };
    net::set_menu_open(true);
    let fallback = init.opts.clone();
    if let Err(e) = std::thread::Builder::new().name("ts3online-menu".into()).spawn(move || run(init, build)) {
        log!("menu: cannot start the thread: {e}");
        net::set_menu_open(false);
        net::start(&Net::Auto, fallback);
    }
}

fn run(init: Init, build: u64) {
    log!("menu: shown");
    let Init { ini, mut opts, last } = init;
    let Some(picked) = (unsafe { show(&opts, &last, build) }) else {
        log!("menu: no window; finding a host automatically");
        net::start(&Net::Auto, opts);
        return net::set_menu_open(false);
    };
    let joined = match &picked.choice {
        Choice::Host => {
            log!("menu: host");
            None
        }
        Choice::Join(a) => {
            log!("menu: join {a}");
            Some(a.clone())
        }
    };
    let changes = model::ini_changes(&opts.name, &opts.key, &last, &picked.name, &picked.key, joined.as_deref());
    if let (false, Some(path)) = (changes.is_empty(), &ini) {
        save_settings(path, &changes);
    }
    let mode = model::to_net(&picked.choice, opts.port);
    opts.name = picked.name;
    opts.key = NetKey::new(&picked.key).as_str().to_string();
    net::start(&mode, opts);
    net::set_menu_open(false);
}

fn save_settings(path: &std::path::Path, changes: &[(&'static str, String)]) {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return log!("menu: cannot read {} to save settings: {e}", path.display()),
    };
    let set: Vec<(&str, &str)> = changes.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let new = model::update_ini(&old, &set);
    if new == old {
        return;
    }
    match std::fs::write(path, new) {
        Ok(()) => log!("menu: saved settings ({})", changes.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")),
        Err(e) => log!("menu: cannot save settings to {}: {e}", path.display()),
    }
}

// ---- discovery ----

/// Everything the timer needs; no window calls in here.
struct App {
    sock: UdpSocket,
    build: u64,
    port: u16,
    peers: Vec<String>,
    table: Table,
    /// tailnet peers and `net_peers` resolved by the background thread
    extra: Arc<Mutex<Vec<SocketAddr>>>,
    busy: Arc<AtomicBool>,
    began: Instant,
    next_probe: u64,
    next_tailnet: u64,
    /// the list as shown, to leave the listbox alone when nothing changed
    shown: Vec<(SocketAddr, String)>,
    status: String,
    /// a message the player must see (kept until the next action)
    notice: Option<String>,
    /// this PC's addresses as the "This PC" line, found by the background thread
    own: Arc<Mutex<Option<String>>>,
    own_shown: Option<String>,
    /// the Join being checked
    pending: Option<Pending>,
    /// a game that did not answer: joining it again goes ahead without the check
    retry: Option<String>,
}

/// A Join waiting for the host's answers.
struct Pending {
    choice: Choice,
    text: String,
    addr: SocketAddr,
    key: String,
    nonce: u64,
    next_send: u64,
    check: model::Check,
}

/// What a timer tick wants the window to change.
struct Update {
    lines: Option<Vec<(SocketAddr, String)>>,
    status: Option<String>,
    own: Option<String>,
    /// the checked Join may go ahead
    join: Option<Choice>,
}

impl App {
    fn new(opts: &net::Options, build: u64) -> std::io::Result<App> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.set_broadcast(true)?;
        sock.set_nonblocking(true)?;
        Ok(App {
            sock,
            build,
            port: opts.port,
            peers: opts.peers.clone(),
            table: Table::default(),
            extra: Arc::default(),
            busy: Arc::default(),
            began: Instant::now(),
            next_probe: 0,
            next_tailnet: 0,
            shown: Vec::new(),
            status: String::new(),
            notice: None,
            own: Arc::default(),
            own_shown: None,
            pending: None,
            retry: None,
        })
    }

    fn drain(&mut self, now: u64) {
        let mut buf = [0u8; 1500];
        // an ICMP "port unreachable" from a dead target shows up as a receive error on Windows: skip those
        for _ in 0..64 {
            match self.sock.recv_from(&mut buf) {
                Ok((n, from)) => self.heard(from, &buf[..n], now),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => continue,
            }
        }
    }

    fn heard(&mut self, from: SocketAddr, bytes: &[u8], now: u64) {
        let key = self.pending.as_ref().map_or(String::new(), |p| p.key.clone());
        let checking = self.pending.as_mut().filter(|p| p.addr == from);
        match proto::decode(bytes, key.as_bytes()) {
            Ok(msg) => {
                if let Some((name, players, locked)) = model::host_info(&msg, self.build) {
                    self.table.heard(from, &name, players, locked, now);
                }
                let Some(p) = checking else { return };
                match msg {
                    Msg::ProbeReply { build, host, players, .. } => p.check.reply = Some((build, host, players)),
                    Msg::CheckReply { nonce } if nonce == p.nonce => p.check.key_ok = true,
                    Msg::Refuse { reason } if reason == proto::REFUSE_KEY => p.check.key_refused = true,
                    _ => {}
                }
            }
            Err(DecodeError::Version(_)) => {
                if let Some(v) = proto::refused_version(bytes) {
                    self.table.heard_version(from, v, now);
                    if let Some(p) = checking {
                        p.check.version = Some(v);
                    }
                }
            }
            Err(_) => {}
        }
    }

    /// Starts checking a Join of `choice` (whose address is `text`) with password `key`.
    fn check(&mut self, choice: Choice, text: String, addr: SocketAddr, key: &str) {
        let now = self.began.elapsed().as_millis() as u64;
        let nonce = (self.began.elapsed().as_nanos() as u64) ^ (u64::from(std::process::id()) << 32);
        let key = NetKey::new(key).as_str().to_string();
        self.pending = Some(Pending { choice, text, addr, key, nonce, next_send: now, check: model::Check { started_ms: now, ..model::Check::default() } });
    }

    /// Sends the pending check when due and decides it.
    fn poll_check(&mut self, now: u64) -> Option<Choice> {
        let p = self.pending.as_mut()?;
        if now >= p.next_send {
            p.next_send = now + model::CHECK_RESEND_MS;
            let _ = self.sock.send_to(&proto::encode(&Msg::Probe { build: self.build }, &[]), p.addr);
            let _ = self.sock.send_to(&proto::encode(&Msg::Check { nonce: p.nonce }, p.key.as_bytes()), p.addr);
        }
        match p.check.verdict(now, self.build, &p.text) {
            Verdict::Wait => None,
            Verdict::Join => self.pending.take().map(|p| p.choice),
            Verdict::Fail(text) => {
                let p = self.pending.take()?;
                let silent = p.check.reply.is_none() && p.check.version.is_none() && !p.check.key_refused;
                self.retry = silent.then_some(p.text);
                self.notice = Some(text);
                None
            }
        }
    }

    fn targets(&self) -> Vec<SocketAddr> {
        let mut t = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), SocketAddr::from((Ipv4Addr::BROADCAST, self.port))];
        if let Ok(extra) = self.extra.lock() {
            for a in extra.iter() {
                if !t.contains(a) {
                    t.push(*a);
                }
            }
        }
        t
    }

    fn refresh_tailnet(&self) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let (extra, busy, own, peers, port) = (self.extra.clone(), self.busy.clone(), self.own.clone(), self.peers.clone(), self.port);
        let spawned = std::thread::Builder::new().name("ts3online-menu-tailnet".into()).spawn({
            let (extra, busy) = (extra.clone(), busy.clone());
            move || {
                let mut found: Vec<SocketAddr> = peers.iter().filter_map(|p| discover::resolve_v4(p, port)).collect();
                let tailnet = discover::tailscale_status().and_then(|j| discover::parse_tailscale(&j));
                if let Some(t) = &tailnet {
                    found.extend(t.peers.iter().map(|ip| SocketAddr::from((*ip, port))));
                }
                let lan = discover::local_ip_towards(SocketAddr::from(ROUTE_PROBE));
                if let Ok(mut g) = own.lock() {
                    *g = Some(model::own_line(tailnet.and_then(|t| t.own), lan, port));
                }
                if let Ok(mut g) = extra.lock() {
                    *g = found;
                }
                busy.store(false, Ordering::Release);
            }
        });
        if spawned.is_err() {
            busy.store(false, Ordering::Release);
        }
    }

    fn poll(&mut self) -> Update {
        let now = self.began.elapsed().as_millis() as u64;
        self.drain(now);
        self.table.expire(now);
        if now >= self.next_tailnet {
            self.next_tailnet = now + TAILNET_EVERY_MS;
            self.refresh_tailnet();
        }
        if now >= self.next_probe {
            self.next_probe = now + PROBE_EVERY_MS;
            let bytes = proto::encode(&Msg::Probe { build: self.build }, &[]);
            for t in self.targets() {
                let _ = self.sock.send_to(&bytes, t);
            }
        }
        let join = self.poll_check(now);
        let own = self.own.lock().ok().and_then(|g| g.clone()).filter(|o| self.own_shown.as_ref() != Some(o));
        if own.is_some() {
            self.own_shown = own.clone();
        }
        let lines = self.table.lines();
        let lines = (lines != self.shown).then(|| {
            self.shown = lines.clone();
            lines
        });
        let status = self.notice.clone().unwrap_or_else(|| model::status_text(self.shown.len()));
        let status = (status != self.status).then(|| {
            self.status = status.clone();
            status
        });
        Update { lines, status, own, join }
    }
}

// ---- window ----

#[derive(Clone, Copy)]
struct Ui {
    name: HWND,
    key: HWND,
    list: HWND,
    addr: HWND,
    join: HWND,
    host: HWND,
    own: HWND,
    status: HWND,
}

const NO_UI: Ui = Ui { name: null_mut(), key: null_mut(), list: null_mut(), addr: null_mut(), join: null_mut(), host: null_mut(), own: null_mut(), status: null_mut() };

thread_local! {
    static UI: Cell<Ui> = const { Cell::new(NO_UI) };
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static PICKED: RefCell<Option<Picked>> = const { RefCell::new(None) };
}

fn text_of(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

fn set_text(h: HWND, s: &str) {
    unsafe { SetWindowTextW(h, wide(OsStr::new(s)).as_ptr()) };
}

/// The address of the selected list entry, if any.
fn selected() -> Option<SocketAddr> {
    let list = UI.with(Cell::get).list;
    let i = unsafe { SendMessageW(list, LB_GETCURSEL, 0, 0) };
    if i == LB_ERR as isize || i < 0 {
        return None;
    }
    APP.with(|a| a.borrow().as_ref().and_then(|a| a.shown.get(i as usize).map(|(addr, _)| *addr)))
}

fn port() -> u16 {
    APP.with(|a| a.borrow().as_ref().map_or(0, |a| a.port))
}

fn on_timer(hwnd: HWND) {
    let keep = selected();
    let Some(update) = APP.with(|a| a.borrow_mut().as_mut().map(App::poll)) else { return };
    let ui = UI.with(Cell::get);
    if let Some(lines) = update.lines {
        unsafe { SendMessageW(ui.list, LB_RESETCONTENT, 0, 0) };
        for (_, text) in &lines {
            unsafe { SendMessageW(ui.list, LB_ADDSTRING, 0, wide(OsStr::new(text)).as_ptr() as LPARAM) };
        }
        if let Some(i) = keep.and_then(|k| lines.iter().position(|(a, _)| *a == k)) {
            unsafe { SendMessageW(ui.list, LB_SETCURSEL, i, 0) };
        }
    }
    if let Some(s) = update.status {
        set_text(ui.status, &s);
    }
    if let Some(s) = update.own {
        set_text(ui.own, &s);
    }
    if let Some(choice) = update.join {
        finish(hwnd, choice);
    }
}

fn notice(text: &str) {
    APP.with(|a| {
        if let Some(a) = a.borrow_mut().as_mut() {
            a.notice = Some(text.to_string());
            a.status = text.to_string();
        }
    });
    set_text(UI.with(Cell::get).status, text);
}

fn clear_notice() {
    APP.with(|a| {
        if let Some(a) = a.borrow_mut().as_mut() {
            a.notice = None;
            a.pending = None;
        }
    });
}

fn finish(hwnd: HWND, choice: Choice) {
    let ui = UI.with(Cell::get);
    PICKED.with(|p| *p.borrow_mut() = Some(Picked { choice, name: text_of(ui.name), key: text_of(ui.key) }));
    unsafe { DestroyWindow(hwnd) };
}

/// Checks that the host of `choice` answers and takes our password, then joins it (from the
/// timer). A game that did not answer last time is joined at once.
fn try_join(hwnd: HWND, choice: Choice) {
    clear_notice();
    let key = text_of(UI.with(Cell::get).key);
    let blocked = APP.with(|a| a.borrow().as_ref().is_some_and(|a| model::needs_password(&choice, &a.table, &key)));
    if blocked {
        return notice("This game needs a password");
    }
    let Choice::Join(text) = &choice else { return finish(hwnd, choice) };
    let text = text.clone();
    let retry = APP.with(|a| a.borrow_mut().as_mut().is_some_and(|a| a.retry.take().as_ref() == Some(&text)));
    if retry {
        return finish(hwnd, choice);
    }
    let addr = text.rsplit_once(':').and_then(|(h, p)| discover::resolve_v4(h, p.parse().ok()?));
    let Some(addr) = addr else {
        return notice(&format!("Cannot find {text}. Check the address."));
    };
    notice(&format!("Checking {text}..."));
    APP.with(|a| {
        if let Some(a) = a.borrow_mut().as_mut() {
            a.check(choice, text, addr, &key);
        }
    });
}

fn click_join(hwnd: HWND) {
    let typed = text_of(UI.with(Cell::get).addr);
    match model::join_choice(selected(), &typed, port()) {
        Some(c) => try_join(hwnd, c),
        None => notice("Pick a game from the list or type an address"),
    }
}

/// Enter pressed with `focus` (the control that had it) focused.
fn on_enter(hwnd: HWND, focus: HWND) {
    let ui = UI.with(Cell::get);
    if focus == ui.host {
        return finish(hwnd, Choice::Host);
    }
    if focus == ui.join {
        return click_join(hwnd);
    }
    match model::enter_choice(selected(), &text_of(ui.addr), port()) {
        Choice::Host => finish(hwnd, Choice::Host),
        join => try_join(hwnd, join),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            on_timer(hwnd);
            0
        }
        WM_COMMAND => {
            let (id, code) = ((wparam & 0xffff) as i32, ((wparam >> 16) & 0xffff) as u32);
            match (id, code) {
                (ID_JOIN, BN_CLICKED) => click_join(hwnd),
                (ID_HOST, BN_CLICKED) => finish(hwnd, Choice::Host),
                (ID_LIST, LBN_DBLCLK) => click_join(hwnd),
                (ID_LIST, LBN_SELCHANGE) => clear_notice(),
                _ => {}
            }
            0
        }
        WM_ENTER => {
            on_enter(hwnd, lparam as HWND);
            0
        }
        WM_CLOSE => {
            finish(hwnd, Choice::Host);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_ID);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn child(parent: HWND, class: &str, text: &str, style: u32, ex: u32, rect: (i32, i32, i32, i32), id: i32) -> HWND {
    let inst = GetModuleHandleW(null());
    CreateWindowExW(
        ex,
        wide(OsStr::new(class)).as_ptr(),
        wide(OsStr::new(text)).as_ptr(),
        WS_CHILD | WS_VISIBLE | style,
        rect.0,
        rect.1,
        rect.2,
        rect.3,
        parent,
        id as isize as HMENU,
        inst,
        null::<c_void>(),
    )
}

/// Creates the window, runs its message loop until the player chooses, returns the choice.
unsafe fn show(opts: &net::Options, last: &str, build: u64) -> Option<Picked> {
    let app = match App::new(opts, build) {
        Ok(a) => a,
        Err(e) => {
            log!("menu: cannot open the discovery socket: {e}");
            return None;
        }
    };
    let inst = GetModuleHandleW(null());
    let class = wide(OsStr::new(CLASS));
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: inst,
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: (COLOR_BTNFACE + 1) as usize as HBRUSH,
        lpszMenuName: null(),
        lpszClassName: class.as_ptr(),
    };
    if RegisterClassW(&wc) == 0 {
        log!("menu: cannot register the window class");
        return None;
    }
    let style = WS_CAPTION | WS_SYSMENU;
    let mut rc = RECT { left: 0, top: 0, right: CLIENT_W, bottom: CLIENT_H };
    AdjustWindowRectEx(&mut rc, style, 0, WS_EX_TOPMOST);
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    let x = ((GetSystemMetrics(SM_CXSCREEN) - w) / 2).max(0);
    let y = ((GetSystemMetrics(SM_CYSCREEN) - h) / 2).max(0);
    let hwnd = CreateWindowExW(WS_EX_TOPMOST, class.as_ptr(), wide(OsStr::new(TITLE)).as_ptr(), style, x, y, w, h, null_mut(), null_mut(), inst, null());
    if hwnd.is_null() {
        log!("menu: cannot create the window");
        UnregisterClassW(class.as_ptr(), inst);
        return None;
    }

    let tab = WS_TABSTOP;
    let edit_style = tab | ES_AUTOHSCROLL as u32;
    let label = |text: &str, y: i32| child(hwnd, "STATIC", text, SS_LEFT as u32, 0, (16, y + 3, 90, 20), 0);
    label("Name", 14);
    let name = child(hwnd, "EDIT", &opts.name, edit_style, WS_EX_CLIENTEDGE, (110, 14, 274, 24), ID_NAME);
    label("Password", 46);
    let key = child(hwnd, "EDIT", &opts.key, edit_style | ES_PASSWORD as u32, WS_EX_CLIENTEDGE, (110, 46, 274, 24), ID_KEY);
    child(hwnd, "STATIC", "Games found", SS_LEFT as u32, 0, (16, 84, 368, 18), 0);
    let list = child(hwnd, "LISTBOX", "", tab | WS_VSCROLL | WS_BORDER | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT) as u32, WS_EX_CLIENTEDGE, (16, 104, 368, 124), ID_LIST);
    label("Address", 240);
    let addr = child(hwnd, "EDIT", last, edit_style, WS_EX_CLIENTEDGE, (110, 240, 274, 24), ID_ADDR);
    let join = child(hwnd, "BUTTON", "Join", tab | BS_PUSHBUTTON as u32, 0, (16, 280, 176, 30), ID_JOIN);
    let host = child(hwnd, "BUTTON", "Host", tab | BS_PUSHBUTTON as u32, 0, (208, 280, 176, 30), ID_HOST);
    let own = child(hwnd, "STATIC", "This PC: ...", SS_LEFT as u32, 0, (16, 320, 368, 18), 0);
    let status = child(hwnd, "STATIC", "", SS_LEFT as u32, 0, (16, 344, 368, 48), 0);
    SendMessageW(name, EM_SETLIMITTEXT, proto::MAX_NAME, 0);
    SendMessageW(key, EM_SETLIMITTEXT, crate::config::MAX_KEY, 0);
    SendMessageW(addr, EM_SETLIMITTEXT, 128, 0);

    // Segoe UI on every control; the stock GUI font if it is missing
    let mut font: HGDIOBJ = CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(OsStr::new("Segoe UI")).as_ptr());
    let own_font = !font.is_null();
    if !own_font {
        font = GetStockObject(DEFAULT_GUI_FONT);
    }
    let mut c = GetWindow(hwnd, GW_CHILD);
    while !c.is_null() {
        SendMessageW(c, WM_SETFONT, font as WPARAM, 1);
        c = GetWindow(c, GW_HWNDNEXT);
    }

    UI.with(|u| u.set(Ui { name, key, list, addr, join, host, own, status }));
    APP.with(|a| *a.borrow_mut() = Some(app));
    on_timer(hwnd); // first probes and the status line before the window shows
    ShowWindow(hwnd, SW_SHOW);
    SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    BringWindowToTop(hwnd);
    SetForegroundWindow(hwnd);
    SetFocus(name);
    SetTimer(hwnd, TIMER_ID, TIMER_MS, None);

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        if msg.message == WM_KEYDOWN && msg.wParam == VK_RETURN {
            SendMessageW(hwnd, WM_ENTER, 0, msg.hwnd as LPARAM);
            continue;
        }
        if IsDialogMessageW(hwnd, &msg) == 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    UnregisterClassW(class.as_ptr(), inst);
    if own_font {
        DeleteObject(font);
    }
    UI.with(|u| u.set(NO_UI));
    APP.with(|a| *a.borrow_mut() = None);
    PICKED.with(|p| p.borrow_mut().take())
}
