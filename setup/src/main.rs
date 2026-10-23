//! ts3-online setup: a first-run wizard (host or join, then the details) and, once installed, the
//! settings screen to change them later. The same program runs on Windows and on Linux/SteamOS.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod platform;
mod probe;
mod settings;
mod steam;
mod text;

use actions::Step;
use eframe::egui::{self, Color32, RichText};
use probe::{CheckState, Probe, Tailscale};
use settings::{Role, Settings};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const ACCENT: Color32 = Color32::from_rgb(255, 196, 64);
const GOOD: Color32 = Color32::from_rgb(120, 210, 120);
const BAD: Color32 = Color32::from_rgb(240, 110, 100);

/// `--shot <file.png> [--page welcome|role|details|done|settings] [--role host|join|ask]`: render a
/// page off screen, save it and exit (screenshots for the docs and for testing the layout).
struct Shot {
    file: PathBuf,
    page: Option<Page>,
    role: Option<Role>,
    frames: u32,
}

fn shot_args() -> Option<Shot> {
    let a: Vec<String> = std::env::args().collect();
    let val = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    let file = PathBuf::from(val("--shot")?);
    let page = val("--page").and_then(|p| match p.as_str() {
        "welcome" => Some(Page::Welcome),
        "role" => Some(Page::Role),
        "details" => Some(Page::Details),
        "done" => Some(Page::Done),
        "settings" => Some(Page::Settings),
        _ => None,
    });
    let role = val("--role").and_then(|r| match r.as_str() {
        "host" => Some(Role::Host),
        "join" => Some(Role::Join),
        "ask" => Some(Role::Ask),
        _ => None,
    });
    Some(Shot { file, page, role, frames: 0 })
}

/// Without a window: `--apply` (install if needed, then save the settings given), `--uninstall
/// [--delete-settings]`, `--update`. Settings flags: `--role host|join|ask --join ADDR --name NAME
/// --password TEXT --port N --game-dir DIR`. Exit code 1 if a step failed.
fn cli() -> Option<i32> {
    let a: Vec<String> = std::env::args().collect();
    let has = |k: &str| a.iter().any(|x| x == k);
    let val = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    if !(has("--apply") || has("--uninstall") || has("--update") || has("--help")) {
        return None;
    }
    attach_console();
    if has("--help") {
        println!("ts3-online-setup {VERSION}
  --text          the setup as questions in the terminal
  (no arguments)  open the setup window
  --apply [--role host|join|ask] [--join ADDR] [--name NAME] [--password TEXT] [--port N]
  --update
  --uninstall [--delete-settings]
  --game-dir DIR  when Steam's libraries do not list the game");
        return Some(0);
    }
    let game = val("--game-dir").map(PathBuf::from).or_else(platform::find_game);
    let Some(game) = game.filter(|g| actions::game_dir_ok(g)) else {
        eprintln!("Toy Story 3 not found: pass --game-dir \"<game folder>\".");
        return Some(1);
    };
    println!("Game folder: {}", game.display());
    let steps = if has("--uninstall") {
        actions::uninstall(&game, has("--delete-settings"))
    } else if has("--update") {
        match actions::latest_release() {
            Ok((v, _)) if !actions::newer(&v, &actions::installed(&game).unwrap_or_default()) => vec![Step { ok: true, text: format!("Up to date ({v}).") }],
            Ok((_, url)) => match actions::download_release(&url) {
                Ok(r) => actions::update_from_github(&game, &r, &user_name()),
                Err(e) => vec![Step { ok: false, text: e }],
            },
            Err(e) => vec![Step { ok: false, text: e }],
        }
    } else {
        let text = actions::read_config(Some(&game));
        let mut s = if text.trim().is_empty() { Settings::new(&user_name()) } else { Settings::from_ini(&text, &user_name()) };
        match val("--role").as_deref() {
            Some("host") => s.role = Role::Host,
            Some("join") => s.role = Role::Join,
            Some("ask") => s.role = Role::Ask,
            Some(r) => {
                eprintln!("--role is host, join or ask, not {r}");
                return Some(2);
            }
            None => {}
        }
        if let Some(j) = val("--join") {
            s.join = j;
            if val("--role").is_none() {
                s.role = Role::Join;
            }
        }
        if let Some(n) = val("--name") {
            s.name = n;
        }
        if let Some(p) = val("--password") {
            s.password = p;
        }
        if let Some(p) = val("--port").and_then(|p| p.parse().ok()) {
            s.port = p;
        }
        if let Some(p) = s.problem() {
            eprintln!("{p}");
            return Some(2);
        }
        if actions::installed(&game).is_none() {
            actions::install_all(&game, &s)
        } else {
            actions::apply(&game, &s, actions::bundled_release(&game).as_ref())
        }
    };
    for s in &steps {
        println!("{} {}", if s.ok { "ok  " } else { "FAIL" }, s.text);
    }
    Some(if steps.iter().all(|s| s.ok) { 0 } else { 1 })
}

/// A window program has no console; print into the one it was started from.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
}

fn main() -> eframe::Result<()> {
    actions::remove_leftovers();
    if let Some(code) = cli() {
        std::process::exit(code);
    }
    if std::env::args().any(|a| a == "--text") {
        attach_console();
        std::process::exit(text::run(&user_name()));
    }
    let shot = shot_args();
    let mut viewport = egui::ViewportBuilder::default().with_icon(std::sync::Arc::new(icon())).with_title(actions::SETUP_TITLE).with_inner_size([680.0, 600.0]).with_min_inner_size([560.0, 480.0]);
    if shot.is_some() {
        viewport = viewport.with_position([-4000.0, -4000.0]).with_active(false).with_taskbar(false);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    let r = eframe::run_native(actions::SETUP_TITLE, options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx, shot)))));
    if let Err(e) = &r {
        // no desktop or no graphics libraries: the same setup as questions in the terminal
        attach_console();
        eprintln!("The setup window cannot open here ({e}); asking in the terminal instead.
");
        std::process::exit(text::run(&user_name()));
    }
    r
}

/// A gold ring on a dark rounded square.
fn icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 - 31.5, y as f32 - 31.5);
            let r = (fx * fx + fy * fy).sqrt();
            let corner = (fx.abs() - 24.0).max(0.0).hypot((fy.abs() - 24.0).max(0.0));
            let px = if corner > 8.0 {
                [0, 0, 0, 0]
            } else if (17.0..=25.0).contains(&r) {
                [255, 196, 64, 255]
            } else if r < 8.0 {
                [255, 196, 64, 255]
            } else {
                [27, 31, 42, 255]
            };
            rgba[(y * n + x) * 4..][..4].copy_from_slice(&px);
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> std::io::Result<()> {
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(f, img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let rgba: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(std::io::Error::other)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Welcome,
    Role,
    Details,
    Done,
    Settings,
}

enum Job {
    Install,
    Save,
    Update(String),
    Uninstall,
    Tailscale,
}

struct App {
    page: Page,
    game: Option<PathBuf>,
    game_text: String,
    installed: Option<String>,
    /// The version of the release this program came in, when it runs from one.
    bundled: Option<String>,
    settings: Settings,
    saved: Option<Settings>,
    show_password: bool,
    probe: Probe,
    build: u64,
    steps: Vec<Step>,
    busy: Option<(String, Receiver<(Vec<Step>, Option<String>)>)>,
    latest: Option<Receiver<Result<(String, String), String>>>,
    latest_seen: Option<Result<(String, String), String>>,
    confirm_uninstall: bool,
    delete_settings: bool,
    shot: Option<Shot>,
}

fn user_name() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_else(|_| "player".into())
}

fn build_of(game: &PathBuf) -> u64 {
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(game.join(platform::EXE)).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    dinput8::pe::identity(&head[..n]).map(|i| i.build_id()).unwrap_or(0)
}

impl App {
    fn new(ctx: &egui::Context, shot: Option<Shot>) -> App {
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 9.0);
        style.spacing.button_padding = egui::vec2(14.0, 7.0);
        style.visuals.selection.bg_fill = Color32::from_rgb(120, 88, 20);
        style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, ACCENT);
        ctx.set_style(style);
        ctx.set_zoom_factor(1.1);
        let game = platform::find_game();
        let installed = game.as_deref().and_then(actions::installed);
        let bundled = game.as_deref().and_then(actions::bundled_version);
        let text = actions::read_config(game.as_deref());
        let saved = (!text.trim().is_empty()).then(|| Settings::from_ini(&text, &user_name()));
        let settings = saved.clone().unwrap_or_else(|| Settings::new(&user_name()));
        let build = game.as_ref().map_or(0, build_of);
        let probe = Probe::start(build, settings.port);
        let mut settings = settings;
        if let Some(r) = shot.as_ref().and_then(|s| s.role) {
            settings.role = r;
        }
        let page = shot.as_ref().and_then(|s| s.page).unwrap_or(if installed.is_some() { Page::Settings } else { Page::Welcome });
        App {
            page,
            game_text: game.as_ref().map(|g| g.display().to_string()).unwrap_or_default(),
            game,
            installed,
            bundled,
            settings,
            saved,
            show_password: false,
            probe,
            build,
            steps: Vec::new(),
            busy: None,
            latest: None,
            latest_seen: None,
            confirm_uninstall: false,
            delete_settings: false,
            shot,
        }
    }

    fn start(&mut self, job: Job) {
        let Some(game) = self.game.clone() else { return };
        let s = self.settings.clone();
        let (label, f): (&str, Box<dyn FnOnce() -> (Vec<Step>, Option<String>) + Send>) = match job {
            Job::Install => ("Installing...", Box::new(move || (actions::install_all(&game, &s), actions::installed(&game)))),
            Job::Save => ("Saving...", Box::new(move || (actions::apply(&game, &s, actions::bundled_release(&game).as_ref()), actions::installed(&game)))),
            Job::Tailscale => ("Getting Tailscale...", Box::new(move || (vec![actions::install_tailscale()], actions::installed(&game)))),
            Job::Update(url) => (
                "Updating...",
                Box::new(move || {
                    let steps = match actions::download_release(&url) {
                        Ok(r) => actions::update_from_github(&game, &r, &user_name()),
                        Err(e) => vec![Step { ok: false, text: e }],
                    };
                    (steps, actions::installed(&game))
                }),
            ),
            Job::Uninstall => {
                let delete = self.delete_settings;
                ("Removing...", Box::new(move || (actions::uninstall(&game, delete), actions::installed(&game))))
            }
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.busy = Some((label.to_string(), rx));
        self.steps.clear();
    }

    fn poll(&mut self) {
        if let Some((_, rx)) = &self.busy {
            if let Ok((steps, installed)) = rx.try_recv() {
                let was_installing = self.page != Page::Settings;
                let saved_ok = steps.iter().any(|s| s.ok && s.text.starts_with("Saved your settings"));
                self.steps = steps;
                self.installed = installed;
                if saved_ok {
                    self.saved = Some(self.settings.clone());
                }
                if was_installing {
                    self.page = Page::Done;
                }
                if self.installed.is_none() && self.page == Page::Settings {
                    self.page = Page::Welcome;
                }
                self.busy = None;
            }
        }
        if let Some(rx) = &self.latest {
            if let Ok(r) = rx.try_recv() {
                self.latest_seen = Some(r);
                self.latest = None;
            }
        }
        let mut targets: Vec<String> = self.settings.recent.clone();
        targets.push(self.settings.join.clone());
        let targets = targets.iter().filter_map(|t| dinput8::menu_model::normalize_address(t, self.settings.port)).collect();
        self.probe.set_targets(self.settings.port, targets);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if let Some(shot) = self.shot.as_mut() {
            shot.frames += 1;
            // a few seconds for the probe and Tailscale to fill in
            if shot.frames == 40 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            let image = ctx.input(|i| i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
            if let Some(img) = image {
                if let Err(e) = save_png(&shot.file, &img) {
                    eprintln!("cannot save {}: {e}", shot.file.display());
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        egui::TopBottomPanel::top("title").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("ts3-online").size(22.0).strong().color(ACCENT));
                ui.label(RichText::new(format!("Online co-op for Toy Story 3 · setup {VERSION}")).weak());
            });
            ui.add_space(4.0);
        });
        egui::TopBottomPanel::bottom("nav").show(ctx, |ui| {
            ui.add_space(6.0);
            self.nav(ui);
            ui.add_space(6.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let step = match self.page {
                Page::Welcome => Some((1, "Your game")),
                Page::Role => Some((2, "Host or join")),
                Page::Details => Some((3, "Details")),
                _ => None,
            };
            if let Some((n, what)) = step {
                ui.label(RichText::new(format!("STEP {n} OF 3 · {}", what.to_uppercase())).size(12.0).color(ACCENT));
            }
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| match self.page {
                Page::Welcome => self.welcome(ui),
                Page::Role => self.role(ui),
                Page::Details => self.details(ui),
                Page::Done => self.done(ui),
                Page::Settings => self.settings_page(ui),
            });
        });
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(19.0).strong());
    ui.add_space(2.0);
}

fn steps_list(ui: &mut egui::Ui, steps: &[Step]) {
    for s in steps {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(if s.ok { "✔" } else { "✖" }).color(if s.ok { GOOD } else { BAD }));
            ui.label(&s.text);
        });
    }
}

impl App {
    fn nav(&mut self, ui: &mut egui::Ui) {
        if let Some((label, _)) = &self.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(label.as_str());
            });
            return;
        }
        let problem = self.settings.problem();
        // (back, message, primary label and whether it is enabled)
        let (back, note, primary): (Option<Page>, Option<(String, Color32)>, Option<(&str, bool)>) = match self.page {
            Page::Welcome => (None, None, Some(("Next  ›", self.game.is_some()))),
            Page::Role => (Some(Page::Welcome), None, Some(("Next  ›", true))),
            Page::Details => (Some(Page::Role), problem.clone().map(|p| (p, BAD)), Some(("Install", problem.is_none()))),
            Page::Done => (None, None, Some(("Close", true))),
            Page::Settings => {
                let changed = self.saved.as_ref() != Some(&self.settings);
                let note = problem.clone().map(|p| (p, BAD)).or_else(|| changed.then(|| ("Not saved yet".to_string(), ACCENT)));
                (None, note, Some((if changed { "Save changes" } else { "Save" }, problem.is_none())))
            }
        };
        ui.horizontal(|ui| {
            if let Some(b) = back {
                if ui.button("‹  Back").clicked() {
                    self.page = b;
                }
            }
            if self.page == Page::Done && self.installed.is_some() && ui.button("Settings").clicked() {
                self.steps.clear();
                self.page = Page::Settings;
            }
            if self.page == Page::Settings && ui.button("Close").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((label, enabled)) = primary {
                    let b = egui::Button::new(RichText::new(label).strong().color(if enabled { Color32::BLACK } else { Color32::GRAY })).fill(if enabled { ACCENT } else { ui.visuals().widgets.inactive.bg_fill });
                    if ui.add_enabled(enabled, b).clicked() {
                        match self.page {
                            Page::Welcome => self.page = Page::Role,
                            Page::Role => self.page = Page::Details,
                            Page::Details => self.start(Job::Install),
                            Page::Done => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                            Page::Settings => self.start(Job::Save),
                        }
                    }
                }
                if let Some((text, color)) = note {
                    ui.label(RichText::new(text).color(color));
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        heading(ui, "Play Toy Story 3 together, each on your own PC");
        ui.label("This sets up ts3-online in a few steps: say whether you host or join a friend, fill in the details, and you are done. Afterwards you just start the game from Steam.");
        ui.add_space(8.0);
        heading(ui, "Your game");
        match &self.game {
            Some(g) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("✔").color(GOOD));
                    ui.label(format!("Found Toy Story 3 in {}", g.display()));
                });
            }
            None => {
                ui.label(RichText::new("Toy Story 3 was not found in your Steam libraries. Paste its folder (Steam: right-click the game, Manage, Browse local files):").color(BAD));
            }
        }
        ui.horizontal(|ui| {
            ui.label("Game folder");
            let r = ui.add(egui::TextEdit::singleline(&mut self.game_text).desired_width(f32::INFINITY));
            if r.changed() {
                let p = PathBuf::from(self.game_text.trim());
                self.game = actions::game_dir_ok(&p).then_some(p);
                if let Some(g) = &self.game {
                    // the folder Steam did not list may already have ts3-online and its settings
                    self.installed = actions::installed(g);
                    self.bundled = actions::bundled_version(g);
                    let text = actions::read_config(Some(g));
                    if !text.trim().is_empty() {
                        let s = Settings::from_ini(&text, &user_name());
                        self.settings = s.clone();
                        self.saved = Some(s);
                    }
                    if self.installed.is_some() {
                        self.page = Page::Settings;
                    }
                    self.build = build_of(g);
                    self.probe = Probe::start(self.build, self.settings.port);
                }
            }
        });
        ui.add_space(8.0);
        heading(ui, "Playing over the internet");
        self.tailscale_line(ui);
        ui.label(RichText::new("Players on the same home network find each other without it.").weak());
    }

    fn tailscale_line(&mut self, ui: &mut egui::Ui) {
        let v = self.probe.view();
        let mut ask_tailscale = false;
        ui.horizontal_wrapped(|ui| match v.tailscale {
            Tailscale::Unknown => {
                ui.spinner();
                ui.label("Checking Tailscale...");
            }
            Tailscale::Missing => {
                ui.label(RichText::new("!").color(ACCENT));
                if platform::tailscale_installed() {
                    ui.label("Tailscale is installed but not signed in: open it and sign in. Everyone signs in to the same tailnet.");
                } else {
                    ui.label("Tailscale is not installed. Friends in other places need it (free); friends on your home network do not.");
                    ask_tailscale = true;
                }
            }
            Tailscale::Connected { own, peers } => {
                ui.label(RichText::new("✔").color(GOOD));
                ui.label(format!("Tailscale is connected ({own}), {peers} other device(s) online."));
            }
        });
        if ask_tailscale && self.busy.is_none() {
            let label = if cfg!(windows) { "Install Tailscale" } else { "Get Tailscale" };
            ui.horizontal_wrapped(|ui| {
                if ui.button(label).clicked() {
                    self.start(Job::Tailscale);
                }
                ui.label(RichText::new(if cfg!(windows) { "downloads Tailscale's own installer from tailscale.com and starts it; it asks for administrator rights" } else { "opens tailscale.com/download for your system" }).weak());
            });
        }
    }

    fn role(&mut self, ui: &mut egui::Ui) {
        heading(ui, "How do you want to play?");
        let choices = [
            (Role::Host, "Host a game", "Your friends join you. Your game starts hosting by itself every time; you play your own save."),
            (Role::Join, "Join a friend", "Your game joins your friend's game every time it starts. You play the host's save; yours is not touched."),
            (Role::Ask, "Choose each time", "A small window opens when the game starts: host, or pick a friend's game."),
        ];
        for (role, title, text) in choices {
            let selected = self.settings.role == role;
            let frame = egui::Frame::group(ui.style()).stroke(egui::Stroke::new(if selected { 2.0_f32 } else { 1.0 }, if selected { ACCENT } else { ui.visuals().widgets.noninteractive.bg_stroke.color }));
            let r = frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).size(17.0).strong().color(if selected { ACCENT } else { ui.visuals().text_color() }));
                    ui.label(text);
                })
                .response
                .interact(egui::Sense::click());
            if r.clicked() {
                self.settings.role = role;
            }
            ui.add_space(4.0);
        }
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        heading(ui, match self.settings.role {
            Role::Host => "Hosting",
            Role::Join => "Joining a friend",
            Role::Ask => "Choosing each time",
        });
        self.details_form(ui);
    }

    fn details_form(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("who").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
            ui.label("Your name");
            ui.add(egui::TextEdit::singleline(&mut self.settings.name).hint_text("shown to the other players").desired_width(260.0));
            ui.end_row();
            ui.label("Password");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.settings.password).password(!self.show_password).hint_text("optional; everyone uses the same").desired_width(260.0));
                ui.checkbox(&mut self.show_password, "show");
            });
            ui.end_row();
            if self.settings.role == Role::Join {
                ui.label("Host's address");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.settings.join).hint_text("pick below, or type e.g. 100.64.0.7").desired_width(260.0));
                    let addr = self.settings.join_address();
                    let running = matches!(self.probe.view().check, Some(CheckState::Running(_)));
                    if ui.add_enabled(addr.is_some() && !running, egui::Button::new("Test")).clicked() {
                        if let Some(a) = addr {
                            self.probe.check(&a, &self.settings.password);
                        }
                    }
                });
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        match self.settings.role {
            Role::Host => self.host_form(ui),
            Role::Join => self.join_form(ui),
            Role::Ask => {
                ui.label("When the game starts, the ts3-online window lists the games it finds: click Host, or pick one and Join. Games you joined before are remembered.");
            }
        }
        egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Port (UDP)");
                let mut text = self.settings.port.to_string();
                if ui.add(egui::TextEdit::singleline(&mut text).desired_width(70.0)).changed() {
                    if let Ok(p) = text.trim().parse::<u16>() {
                        if p > 0 {
                            self.settings.port = p;
                        }
                    }
                }
                ui.label(RichText::new("every player uses the same; 27777 unless you have a reason").weak());
            });
        });
    }

    fn host_form(&mut self, ui: &mut egui::Ui) {
        let v = self.probe.view();
        heading(ui, "Your friends join you at");
        let mut lines = Vec::new();
        if let Tailscale::Connected { own, .. } = v.tailscale {
            lines.push((own.to_string(), "Tailscale, for friends anywhere"));
        }
        if let Some(lan) = v.lan {
            lines.push((lan.to_string(), "this network, for players in the same home"));
        }
        if lines.is_empty() {
            ui.label("Looking up your addresses...");
        }
        for (addr, what) in lines {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&addr).monospace().strong());
                if ui.small_button("Copy").clicked() {
                    ui.output_mut(|o| o.copied_text = addr.clone());
                }
                ui.label(RichText::new(what).weak());
            });
        }
        ui.label(RichText::new("Most friends do not need it: your game appears in their list by itself.").weak());
        if cfg!(windows) {
            ui.label(RichText::new("The first time you host, Windows asks whether to let the game through its firewall: click Allow.").weak());
        }
    }

    fn join_form(&mut self, ui: &mut egui::Ui) {
        let v = self.probe.view();
        match &v.check {
            Some(CheckState::Running(a)) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Asking {a}..."));
                });
            }
            Some(CheckState::Done { addr, ok, text }) if self.settings.join_address().as_deref() == Some(addr.as_str()) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(if *ok { "✔" } else { "✖" }).color(if *ok { GOOD } else { BAD }));
                    ui.label(text);
                });
            }
            _ => {}
        }
        ui.add_space(4.0);
        heading(ui, "Games found");
        if v.games.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Looking on your tailnet and network... the host's game must be running and hosting.").weak());
            });
        }
        let port = self.settings.port;
        for g in &v.games {
            let label = format!("{}   {}", g.line, if self.settings.join_address().as_deref() == Some(g.addr.as_str()) { "✔" } else { "" });
            if ui.selectable_label(false, label).clicked() {
                self.settings.join = strip_port(&g.addr, port);
            }
        }
        if !self.settings.recent.is_empty() {
            heading(ui, "Joined before");
            let mut remove = None;
            for (i, r) in self.settings.recent.iter().enumerate() {
                let full = dinput8::menu_model::normalize_address(r, port).unwrap_or_default();
                let online = v.games.iter().any(|g| g.addr == full);
                ui.horizontal(|ui| {
                    if ui.selectable_label(false, format!("{r}   {}", if online { "(online now)" } else { "" })).clicked() {
                        self.settings.join = r.clone();
                    }
                    if ui.small_button("Forget").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.settings.recent.remove(i);
            }
        }
        ui.add_space(4.0);
        ui.label(RichText::new("If the host's game does not answer when you start, the ts3-online window opens so you can pick another or keep trying.").weak());
    }

    fn done(&mut self, ui: &mut egui::Ui) {
        let all_ok = self.steps.iter().all(|s| s.ok);
        heading(ui, if all_ok { "All set" } else { "Almost there" });
        steps_list(ui, &self.steps);
        ui.add_space(10.0);
        if all_ok {
            ui.label(RichText::new("Start Toy Story 3 from Steam as usual.").size(17.0).strong());
            ui.label(match self.settings.role {
                Role::Host => "Your game hosts by itself; friends' games find it. Load your save and enter Toy Box.",
                Role::Join => "Your game joins the host by itself. Pick a character and enter Toy Box.",
                Role::Ask => "The ts3-online window opens first: Host, or pick a game and Join.",
            });
            let where_ = self.game.as_ref().map(|g| platform::mod_dir(g).display().to_string()).unwrap_or_default();
            ui.label(RichText::new(format!("To change anything later (host instead of join, a new address, your name), run ts3-online-setup again; a copy is in {where_}. Everything the mod uses is in the game folder, so you can move it to another PC.")).weak());
        } else {
            ui.label("Fix what is marked above, then try again from Settings.");
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        if let Some(g) = &self.game {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("✔").color(GOOD));
                let v = self.installed.as_deref().filter(|v| !v.is_empty()).unwrap_or("(older release)");
                ui.label(format!("ts3-online {v} is installed in {}", g.display()));
            });
        }
        if let (Some(b), Some(have)) = (&self.bundled, &self.installed) {
            let have_text = if have.is_empty() { "an older release" } else { have.as_str() };
            if actions::newer(b, have) {
                ui.label(RichText::new(format!("This setup is ts3-online {b} and the game has {have_text}: Save installs {b}, the setup program in the game folder included.")).color(ACCENT));
            } else if actions::newer(have, b) {
                ui.label(RichText::new(format!("The game already has ts3-online {have}, newer than this setup ({b}): Save only changes your settings.")).weak());
            }
        }
        if self.game.as_deref().is_some_and(actions::needs_repair) {
            ui.label(RichText::new("Steam put its own steam_api64.dll back (a file check or an update): Save puts the mod back in its place.").color(ACCENT));
        }
        heading(ui, "How you play");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.settings.role, Role::Host, "Host");
            ui.selectable_value(&mut self.settings.role, Role::Join, "Join a friend");
            ui.selectable_value(&mut self.settings.role, Role::Ask, "Choose each time");
        });
        ui.add_space(4.0);
        self.details_form(ui);
        if !self.steps.is_empty() {
            ui.add_space(6.0);
            steps_list(ui, &self.steps);
        }
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Updates, help and uninstall").show(ui, |ui| self.maintenance(ui));
    }

    fn maintenance(&mut self, ui: &mut egui::Ui) {
        self.tailscale_line(ui);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.latest.is_none(), egui::Button::new("Check for updates")).clicked() {
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let _ = tx.send(actions::latest_release());
                });
                self.latest = Some(rx);
            }
            if self.latest.is_some() {
                ui.spinner();
            }
            match &self.latest_seen {
                Some(Ok((v, url))) => {
                    let have = self.installed.clone().unwrap_or_default();
                    if actions::newer(v, &have) {
                        let url = url.clone();
                        if ui.button(format!("Update to {v}")).clicked() {
                            self.start(Job::Update(url));
                        }
                    } else {
                        ui.label(format!("Up to date ({v})."));
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(BAD));
                }
                None => {}
            }
        });
        if let Some(g) = self.game.clone() {
            ui.horizontal(|ui| {
                if ui.button("Open the ts3-online folder").clicked() {
                    platform::open(&platform::mod_dir(&g).to_string_lossy());
                }
                ui.label(RichText::new("settings, logs (ts3online.log helps with bug reports) and save backups").weak());
            });
        }
        ui.add_space(6.0);
        if !self.confirm_uninstall {
            if ui.button(RichText::new("Uninstall ts3-online...").color(BAD)).clicked() {
                self.confirm_uninstall = true;
            }
        } else {
            ui.label("This removes ts3-online from the game folder; your saves and save backups stay.");
            ui.checkbox(&mut self.delete_settings, "Also delete my settings and the games I joined");
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Uninstall").color(BAD)).clicked() {
                    self.confirm_uninstall = false;
                    self.start(Job::Uninstall);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_uninstall = false;
                }
            });
        }
    }
}

fn strip_port(addr: &str, port: u16) -> String {
    addr.strip_suffix(&format!(":{port}")).unwrap_or(addr).to_string()
}
