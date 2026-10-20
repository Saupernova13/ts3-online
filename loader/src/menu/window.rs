//! The Win32 window: controls, the window procedure and the message loop.
use super::app::App;
use super::Picked;
use crate::log;
use crate::menu_model::{self as model, Choice};
use crate::net::proto;
use crate::net::{self, discover};
use crate::win::wide;
use std::cell::{Cell, RefCell};
use std::ffi::{c_void, OsStr};
use std::net::SocketAddr;
use std::ptr::{null, null_mut};
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

/// Joins once the host answers and takes the password (decided in the timer); a game that did not
/// answer last time is joined at once.
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

/// Runs the window to completion on the calling thread; `None` when it could not open.
pub(super) unsafe fn show(opts: &net::Options, last: &str, build: u64) -> Option<Picked> {
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
    on_timer(hwnd); // first probes and status before the window shows
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
