#[cfg(not(target_os = "macos"))]
compile_error!("ByteBroom only supports macOS.");

mod access;
mod apps;
mod rules;

use eframe::egui;
use egui_phosphor::regular as icons;
use rayon::prelude::*;
use rules::Group;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use trash::macos::{DeleteMethod, TrashContextExtMacos};

const REPO_URL: &str = "https://github.com/p32929/bytebroom";

// Files smaller than this still count toward their folder, but get no row of their own.
const MIN_FILE: u64 = 1 << 20;

pub struct Node {
    name: String,
    size: u64,
    is_dir: bool,
    markers: u32, // which project marker files sit directly in this folder (see rules::marker)
    children: Vec<Node>,
}

// ponytail: hardlinks and APFS clones are counted once per path, same as `du` without
// dedup, so totals can exceed real disk usage. Track inodes if that ever matters.
fn scan(path: &Path, name: String, devs: [u64; 2], seen: &AtomicU64) -> Node {
    let mut node = Node { name, size: 0, is_dir: true, markers: 0, children: Vec::new() };
    let Ok(entries) = std::fs::read_dir(path) else { return node };
    let mut dirs = Vec::new();
    for e in entries.flatten() {
        // DirEntry::metadata does not follow symlinks, so links are never descended into.
        let Ok(md) = e.metadata() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if md.is_dir() {
            let p = e.path();
            // Stay on this disk (skips network mounts and external drives). /System/Volumes
            // holds the same data volume again under another name, so it would double count.
            if devs.contains(&md.dev()) && p != Path::new("/System/Volumes") {
                dirs.push((p, name));
            }
        } else {
            let size = md.blocks() * 512; // allocated size, like `du`
            seen.fetch_add(size, Ordering::Relaxed);
            node.size += size;
            node.markers |= rules::marker(&name);
            if size >= MIN_FILE {
                node.children.push(Node { name, size, is_dir: false, markers: 0, children: Vec::new() });
            }
        }
    }
    let subs: Vec<Node> = dirs.into_par_iter().map(|(p, n)| scan(&p, n, devs, seen)).collect();
    node.size += subs.iter().map(|s| s.size).sum::<u64>();
    node.children.extend(subs);
    node.children.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    node
}

fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// Shows paths inside the home folder as `~/…` so rows stay short.
fn pretty(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

// Folders open in Finder; files are revealed (selected) in their folder.
fn open_in_finder(path: &Path, is_dir: bool) {
    let mut cmd = Command::new("open");
    if !is_dir {
        cmd.arg("-R");
    }
    let _ = cmd.arg(path).spawn();
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Trash,
    Permanent,
}

/// Last line of defence: whatever the rules produced, never remove the home folder, anything
/// above it, or a top-level system folder such as /Library/Caches itself.
fn is_deletable(path: &Path, home: &Path) -> bool {
    // An app bundle directly inside /Applications is the one allowed three-part path.
    let app_bundle = path.starts_with("/Applications") && path.extension().is_some_and(|e| e == "app");
    path.is_absolute() && (path.components().count() >= 4 || app_bundle) && !home.starts_with(path)
}

fn remove(path: &Path, mode: Mode, home: &Path) -> Result<(), String> {
    if !is_deletable(path, home) {
        return Err("refusing to delete a top-level folder".into());
    }
    // symlink_metadata: if the path has become a link, remove the link, never its target.
    let md = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    match mode {
        Mode::Trash => {
            if path.starts_with(home.join(".Trash")) {
                return Err("already in the Trash; use Delete permanently".into());
            }
            let mut ctx = trash::TrashContext::default();
            // The default asks Finder to do it, which needs an extra permission prompt.
            ctx.set_delete_method(DeleteMethod::NsFileManager);
            ctx.delete(path).map_err(|e| e.to_string())
        }
        Mode::Permanent if md.is_dir() => std::fs::remove_dir_all(path)
            .or_else(|_| {
                // Some caches (Go modules) are stored read-only: make them writable and retry.
                let _ = Command::new("chmod").args(["-R", "u+w"]).arg(path).status();
                std::fs::remove_dir_all(path)
            })
            .map_err(|e| e.to_string()),
        Mode::Permanent => std::fs::remove_file(path).map_err(|e| e.to_string()),
    }
}

struct Deleted {
    done: HashSet<PathBuf>,
    freed: u64,
    errors: Vec<String>,
    mode: Mode,
}

/// Live state of a running delete, shared with the worker thread and shown in a dialog.
// ponytail: progress moves per ticked item, so one huge folder sits still until it is done.
// Walk and count files inside each item if that ever feels stuck.
#[derive(Default)]
struct Progress {
    current: String, // item being removed right now
    items: (usize, usize), // (done, total)
    bytes: (u64, u64),     // (done, total)
    failed: usize,
}

#[derive(PartialEq)]
enum View {
    Clean,
    Apps,
}

struct App {
    home: PathBuf,
    groups: Option<Vec<Group>>, // None until the first scan finishes
    view: View,
    selected: Option<String>,     // clean-up group shown on the right
    selected_app: Option<String>, // app shown on the right
    filter: String,               // text typed into "Find an app"
    rx: Option<mpsc::Receiver<Vec<Group>>>,
    seen: Arc<AtomicU64>,
    confirming: bool,
    about: bool, // the About dialog, opened from the version number
    deleting: Option<mpsc::Receiver<Deleted>>,
    progress: Arc<Mutex<Progress>>,
    status: Option<String>,
    delete_errors: Vec<String>,
    needs_access: bool, // no Full Disk Access yet: explain it before the first scan
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        apply_style(ctx);
        // Follows the macOS appearance unless started with --light or --dark.
        let mut start_in_apps = false; // --apps opens on the Apps view
        let mut show = None; // --show=<group or app name> opens on that entry
        for arg in std::env::args() {
            match arg.as_str() {
                "--light" => ctx.set_theme(egui::ThemePreference::Light),
                "--dark" => ctx.set_theme(egui::ThemePreference::Dark),
                "--apps" => start_in_apps = true,
                a => show = a.strip_prefix("--show=").map(String::from).or(show),
            }
        }
        let mut app = App {
            home: home_dir(),
            groups: None,
            view: if start_in_apps { View::Apps } else { View::Clean },
            selected: show.clone(),
            selected_app: show,
            filter: String::new(),
            rx: None,
            seen: Arc::new(AtomicU64::new(0)),
            confirming: false,
            about: false,
            deleting: None,
            progress: Arc::default(),
            status: None,
            delete_errors: Vec::new(),
            needs_access: !access::has_full_disk_access(),
        };
        // With the explanation showing, scanning waits: it is what makes macOS ask folder by folder.
        if !app.needs_access {
            app.start_scan(ctx);
        }
        app
    }

    fn access_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.2);
                ui.label(egui::RichText::new(icons::SHIELD_CHECK).size(48.0));
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Allow one permission, once").size(24.0).strong());
                ui.add_space(8.0);
                ui.allocate_ui(egui::vec2(480.0, 0.0), |ui| {
                    ui.label("ByteBroom looks in places macOS protects, like caches, app data and your Documents, Desktop and Downloads folders.");
                    ui.add_space(6.0);
                    ui.label("Without Full Disk Access, macOS asks about each of those separately while it scans, and ByteBroom still finds less. Turn it on once and it stops asking.");
                    ui.add_space(6.0);
                    ui.weak("ByteBroom only reads sizes and names. Nothing leaves your Mac, and nothing is deleted until you choose it.");
                });
                ui.add_space(18.0);
                let open = egui::Button::new(egui::RichText::new("Open Full Disk Access settings").strong());
                if ui.add(open).clicked() {
                    access::open_settings();
                }
                ui.add_space(4.0);
                ui.weak("Switch on ByteBroom in the list (use + if it isn't there), then come back.");
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    if access::can_relaunch() {
                        if ui.button("I turned it on: restart ByteBroom").clicked() {
                            access::relaunch();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    } else if ui.button("Quit, then start ByteBroom again").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Scan without it").clicked() {
                        self.needs_access = false;
                        self.start_scan(ctx);
                    }
                });
            });
        });
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        self.groups = None;
        self.status = None;
        self.delete_errors.clear();
        self.seen = Arc::new(AtomicU64::new(0));
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let (seen, ctx, home) = (self.seen.clone(), ctx.clone(), self.home.clone());
        std::thread::spawn(move || {
            let _ = tx.send(scan_disk(&home, &seen));
            ctx.request_repaint();
        });
    }

    fn start_delete(&mut self, mode: Mode, ctx: &egui::Context) {
        let Some(groups) = &self.groups else { return };
        // A map, because one folder can be ticked in both views and must be deleted only once.
        let picked: std::collections::BTreeMap<PathBuf, u64> = groups
            .iter()
            .flat_map(|g| &g.items)
            .filter(|f| f.checked)
            .map(|f| (f.path.clone(), f.size))
            .collect();
        let (tx, rx) = mpsc::channel();
        self.deleting = Some(rx);
        self.progress = Arc::new(Mutex::new(Progress {
            items: (0, picked.len()),
            bytes: (0, picked.values().sum()),
            ..Default::default()
        }));
        let (home, ctx, progress) = (self.home.clone(), ctx.clone(), self.progress.clone());
        std::thread::spawn(move || {
            let mut out = Deleted { done: HashSet::new(), freed: 0, errors: Vec::new(), mode };
            for (path, size) in picked {
                progress.lock().unwrap().current = pretty(&path, &home);
                ctx.request_repaint();
                let result = remove(&path, mode, &home);
                let mut p = progress.lock().unwrap();
                p.items.0 += 1;
                p.bytes.0 += size; // counted even on failure, so the bar still reaches the end
                match result {
                    Ok(()) => {
                        out.freed += size;
                        out.done.insert(path);
                    }
                    Err(e) => {
                        p.failed += 1;
                        out.errors.push(format!("{}: {e}", pretty(&path, &home)));
                    }
                }
            }
            let _ = tx.send(out);
            ctx.request_repaint();
        });
    }

    fn finish_delete(&mut self, d: Deleted) {
        if let Some(groups) = &mut self.groups {
            for g in groups.iter_mut() {
                g.items.retain(|f| !d.done.contains(&f.path));
                g.total = g.items.iter().map(|f| f.size).sum();
            }
            groups.retain(|g| !g.items.is_empty());
        }
        let what = match d.mode {
            Mode::Trash => "Moved to Trash",
            Mode::Permanent => "Deleted",
        };
        let tail = match d.mode {
            Mode::Trash => " Empty the Trash to get the space back.",
            Mode::Permanent => "",
        };
        self.status = Some(format!("{what}: {} items, {}.{tail}", d.done.len(), human(d.freed)));
        self.delete_errors = d.errors;
    }
}

/// Every colour the app uses, per theme. Zinc neutrals; colour is reserved for meaning:
/// green = safe, amber = look first, red = destructive.
struct Palette {
    bg: egui::Color32,
    side: egui::Color32, // sidebar and bottom bar, a step away from the content
    field: egui::Color32,
    text: egui::Color32,
    weak: egui::Color32,
    line: egui::Color32,
    widget: egui::Color32,
    hover: egui::Color32,
    select: egui::Color32,
    green: egui::Color32,
    amber: egui::Color32,
    red: egui::Color32,
}

const fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const DARK: Palette = Palette {
    bg: rgb(0x18181b),
    side: rgb(0x131315),
    field: rgb(0x101012),
    text: rgb(0xe4e4e7),
    weak: rgb(0xa1a1aa),
    line: rgb(0x2e2e33),
    widget: rgb(0x2a2a2f),
    hover: rgb(0x3a3a41),
    select: rgb(0x1f4d36),
    green: rgb(0x5fcf8f),
    amber: rgb(0xe5a93d),
    red: rgb(0xe5484d),
};

const LIGHT: Palette = Palette {
    bg: rgb(0xfafafa),
    side: rgb(0xf0f0f2),
    field: rgb(0xf0f0f1),
    text: rgb(0x18181b),
    weak: rgb(0x52525b),
    line: rgb(0xdcdce0),
    widget: rgb(0xe7e7ea),
    hover: rgb(0xd6d6db),
    select: rgb(0xbfe8d0),
    green: rgb(0x15803d),
    amber: rgb(0xa8540a),
    red: rgb(0xd32f2f),
};

fn palette(ui: &egui::Ui) -> &'static Palette {
    if ui.visuals().dark_mode { &DARK } else { &LIGHT }
}

/// Fonts, sizes, spacing and colours for both themes. Called once at startup.
fn apply_style(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};

    // Use the macOS system fonts when present; egui's built-in fonts are the fallback.
    let mut fonts = egui::FontDefinitions::default();
    for (name, path, family) in [
        ("sf", "/System/Library/Fonts/SFNS.ttf", FontFamily::Proportional),
        ("sf-mono", "/System/Library/Fonts/SFNSMono.ttf", FontFamily::Monospace),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    }
    // Icon font as a fallback, so icons can sit inside ordinary text.
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    egui_extras::install_image_loaders(ctx); // lets app icons load from PNG files

    for (theme, p) in [(egui::Theme::Dark, &DARK), (egui::Theme::Light, &LIGHT)] {
        ctx.style_mut_of(theme, |s| {
            s.text_styles = [
                (TextStyle::Heading, FontId::proportional(22.0)),
                (TextStyle::Body, FontId::proportional(15.0)),
                (TextStyle::Button, FontId::proportional(15.0)),
                (TextStyle::Small, FontId::proportional(12.5)),
                (TextStyle::Monospace, FontId::monospace(14.0)),
            ]
            .into();
            s.spacing.item_spacing = egui::vec2(10.0, 8.0);
            s.spacing.button_padding = egui::vec2(12.0, 6.0);
            s.spacing.interact_size.y = 30.0;
            s.spacing.icon_width = 18.0;
            s.spacing.indent = 28.0;

            let v = &mut s.visuals;
            v.panel_fill = p.bg;
            v.window_fill = p.bg;
            v.extreme_bg_color = p.field;
            v.faint_bg_color = p.field;
            v.weak_text_color = Some(p.weak);
            v.hyperlink_color = p.text;
            v.window_stroke = egui::Stroke::new(1.0, p.line);
            v.window_corner_radius = egui::CornerRadius::same(10);
            v.selection.bg_fill = p.select;
            v.selection.stroke = egui::Stroke::new(1.0, p.text);
            // One corner radius everywhere.
            let radius = egui::CornerRadius::same(6);
            let w = &mut v.widgets;
            for (state, fill) in [
                (&mut w.noninteractive, p.bg),
                (&mut w.inactive, p.widget),
                (&mut w.hovered, p.hover),
                (&mut w.active, p.hover),
                (&mut w.open, p.widget),
            ] {
                state.corner_radius = radius;
                state.bg_fill = fill;
                state.weak_bg_fill = fill;
                state.bg_stroke = egui::Stroke::NONE;
                state.fg_stroke = egui::Stroke::new(1.0, p.text);
                state.expansion = 0.0;
            }
            w.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.line); // separators, panel edges
        });
    }
}

/// (items, bytes, how many of them are "look first") currently ticked.
fn selection(groups: &[Group]) -> (usize, u64, usize) {
    let mut out = (0, 0, 0);
    for g in groups {
        for f in g.items.iter().filter(|f| f.checked) {
            out.0 += 1;
            out.1 += f.size;
            out.2 += usize::from(!g.safe);
        }
    }
    out
}

fn stat(ui: &mut egui::Ui, value: u64, caption: &str, color: egui::Color32) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(egui::RichText::new(human(value)).size(30.0).strong().color(color));
        ui.label(egui::RichText::new(caption).weak());
    });
}

fn size_label(ui: &mut egui::Ui, bytes: u64) {
    // Fixed width and right-aligned so the numbers line up in a column.
    ui.allocate_ui_with_layout(
        egui::vec2(88.0, ui.spacing().interact_size.y),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| ui.label(egui::RichText::new(human(bytes)).monospace()),
    );
}

/// Icon for a clean-up group. Apps use their own icon instead.
fn group_icon(name: &str) -> &'static str {
    match name {
        "Project build folders" => icons::HAMMER,
        "Xcode build data" => icons::CUBE,
        "App caches" => icons::DATABASE,
        "Package manager caches" => icons::PACKAGE,
        "Logs" => icons::SCROLL,
        "Trash" => icons::TRASH,
        "Temporary files" => icons::TIMER,
        "iOS simulators" => icons::DEVICE_MOBILE,
        "Android emulators" => icons::ANDROID_LOGO,
        "Docker and VM data" => icons::HARD_DRIVES,
        "Downloads" => icons::DOWNLOAD_SIMPLE,
        "Backups and archives" => icons::ARCHIVE,
        "Caches worth checking" => icons::MAGNIFYING_GLASS,
        _ => icons::FOLDER,
    }
}

/// The app's own icon when it has one, otherwise a generic glyph, at `size` points.
fn group_image(ui: &mut egui::Ui, g: &Group, size: f32) {
    match &g.icon {
        Some(uri) => {
            ui.add(egui::Image::new(uri.as_str()).fit_to_exact_size(egui::vec2(size, size)));
        }
        None => {
            let glyph = if g.app { icons::APP_WINDOW } else { group_icon(&g.name) };
            ui.add_sized([size, size], egui::Label::new(egui::RichText::new(glyph).size(size * 0.8)));
        }
    }
}

/// One clickable row of the sidebar: icon and name on the left, size on the right.
fn group_row(ui: &mut egui::Ui, g: &Group, selected: &mut Option<String>) {
    let ticked = g.items.iter().filter(|f| f.checked).count();
    let name = if ticked > 0 { format!("{}  ({ticked})", g.name) } else { g.name.clone() };
    let is_selected = selected.as_deref() == Some(g.name.as_str());
    let size = egui::RichText::new(human(g.total)).monospace().small();
    let min = egui::vec2(ui.available_width(), 38.0);
    let button = match &g.icon {
        Some(uri) => {
            let image = egui::Image::new(uri.as_str()).fit_to_exact_size(egui::vec2(22.0, 22.0));
            egui::Button::selectable(is_selected, (image, name))
        }
        None => {
            let glyph = if g.app { icons::APP_WINDOW } else { group_icon(&g.name) };
            egui::Button::selectable(is_selected, format!("{glyph}   {name}"))
        }
    };
    if ui.add(button.right_text(size).truncate().min_size(min)).clicked() {
        *selected = Some(g.name.clone());
    }
}

/// Left side: the view switch, then either the clean-up groups or the list of apps.
fn sidebar_ui(ui: &mut egui::Ui, groups: &mut [Group], app: &mut App) {
    let p = palette(ui);
    ui.spacing_mut().item_spacing.y = 4.0;
    ui.horizontal(|ui| {
        // Two equal halves, like a segmented control.
        let half = egui::vec2((ui.available_width() - ui.spacing().item_spacing.x) / 2.0, 32.0);
        for (view, label) in [
            (View::Clean, format!("{}  Clean up", icons::BROOM)),
            (View::Apps, format!("{}  Apps", icons::SQUARES_FOUR)),
        ] {
            if ui.add(egui::Button::selectable(app.view == view, label).min_size(half)).clicked() {
                app.view = view;
            }
        }
    });
    ui.add_space(12.0);
    if app.view == View::Apps {
        let hint = format!("{}  Find an app", icons::MAGNIFYING_GLASS);
        ui.add(
            egui::TextEdit::singleline(&mut app.filter)
                .hint_text(hint)
                .margin(egui::vec2(10.0, 8.0))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(8.0);
        let wanted = app.filter.to_lowercase();
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut any = false;
            for g in groups.iter_mut().filter(|g| g.app && g.name.to_lowercase().contains(&wanted)) {
                ui.horizontal(|ui| {
                    // Ticks the app and every trace it left, so it is removed fully.
                    let ticked = g.items.iter().filter(|f| f.checked).count();
                    let mut all = ticked == g.items.len();
                    let some = ticked > 0 && !all;
                    let tip = "Select the app and all its files";
                    if ui.add(egui::Checkbox::new(&mut all, "").indeterminate(some)).on_hover_text(tip).changed() {
                        g.items.iter_mut().for_each(|f| f.checked = all);
                    }
                    group_row(ui, g, &mut app.selected_app);
                });
                any = true;
            }
            if !any {
                ui.add_space(8.0);
                ui.weak("No app matches that.");
            }
        });
        return;
    }
    let total = |safe: bool| groups.iter().filter(|g| !g.app && g.safe == safe).map(|g| g.total).sum::<u64>();
    ui.horizontal(|ui| {
        stat(ui, total(true), "safe to delete", p.green);
        ui.add_space(12.0);
        stat(ui, total(false), "look first", p.amber);
    });
    ui.add_space(8.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut section = None;
        for g in groups.iter().filter(|g| !g.app) {
            // Groups arrive safe-first, so each kind gets one heading.
            if section != Some(g.safe) {
                section = Some(g.safe);
                ui.add_space(14.0);
                let (title, color) = if g.safe {
                    (format!("{}  Safe to delete", icons::SHIELD_CHECK), p.green)
                } else {
                    (format!("{}  Look first", icons::WARNING), p.amber)
                };
                ui.label(egui::RichText::new(title).small().strong().color(color));
                ui.add_space(4.0);
            }
            group_row(ui, g, &mut app.selected);
        }
    });
}

/// Right side: the items of one group, with a checkbox on top that ticks or unticks them all.
fn items_ui(ui: &mut egui::Ui, g: &mut Group, home: &Path) -> Option<(PathBuf, bool)> {
    let p = palette(ui);
    let mut open = None;
    ui.horizontal(|ui| {
        group_image(ui, g, 44.0);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.label(egui::RichText::new(&g.name).size(22.0).strong());
            let n = g.items.len();
            ui.weak(format!("{n} {}, {}", if n == 1 { "item" } else { "items" }, human(g.total)));
        });
    });
    ui.add_space(10.0);
    let (note, color) = if g.app {
        ("Tick everything to remove the app and all its traces. Its settings and data are lost.", p.amber)
    } else if g.safe {
        ("Safe to delete: comes back by itself, or with a rebuild or reinstall.", p.green)
    } else {
        ("Look first: you lose what is inside, so check before deleting.", p.amber)
    };
    ui.colored_label(color, note);
    ui.add_space(12.0);
    // A dash means only some are ticked; clicking it ticks everything shown.
    let ticked = g.items.iter().filter(|f| f.checked).count();
    let mut all = ticked == g.items.len();
    let some = ticked > 0 && !all;
    if ui.add(egui::Checkbox::new(&mut all, "Select all").indeterminate(some)).changed() {
        g.items.iter_mut().for_each(|f| f.checked = all);
    }
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 14.0;
        for f in &mut g.items {
            ui.horizontal(|ui| {
                ui.checkbox(&mut f.checked, "");
                size_label(ui, f.size);
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(12.0);
                    let show = egui::Button::new(egui::RichText::new(icons::FOLDER_OPEN).size(17.0));
                    if ui.add(show).on_hover_text("Show in Finder").clicked() {
                        open = Some((f.path.clone(), f.is_dir));
                    }
                    // Path with its reason underneath; both cut off rather than wrap.
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(egui::Label::new(pretty(&f.path, home)).truncate());
                        ui.add(egui::Label::new(egui::RichText::new(f.why).small().weak()).truncate());
                    });
                });
            });
        }
        ui.add_space(8.0);
    });
    open
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let p = palette(ui);
        if self.needs_access {
            self.access_ui(ui, ctx);
            return;
        }
        if let Some(groups) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.groups = Some(groups);
            self.rx = None;
        }
        if let Some(d) = self.deleting.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.deleting = None;
            self.finish_delete(d);
        }
        let scanning = self.rx.is_some();
        let deleting = self.deleting.is_some();

        let (count, bytes, risky) = self.groups.as_deref().map_or((0, 0, 0), selection);
        let bar = egui::Frame::new().fill(p.side).inner_margin(egui::Margin::symmetric(16, 12));
        egui::Panel::bottom("actions").frame(bar).show(ui, |ui| {
            ui.horizontal(|ui| {
                if deleting {
                    ui.spinner();
                    ui.label("Deleting…");
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                } else if count == 0 {
                    ui.add_enabled(false, egui::Button::new(format!("{}  Delete selected", icons::TRASH)));
                    if self.groups.is_some() && self.status.is_none() {
                        ui.weak("Tick the items you want to remove.");
                    }
                } else {
                    let label = format!("{}  Delete {count} selected ({})", icons::TRASH, human(bytes));
                    let text = egui::RichText::new(label).strong().color(egui::Color32::WHITE);
                    if ui.add(egui::Button::new(text).fill(p.red)).clicked() {
                        self.confirming = true;
                    }
                }
                if let Some(s) = &self.status {
                    ui.label(s);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::widgets::global_theme_preference_buttons(ui);
                    ui.separator();
                    let version = egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).small().weak();
                    if ui.add(egui::Button::new(version).frame(false)).on_hover_text("About ByteBroom").clicked() {
                        self.about = true;
                    }
                    if ui.add_enabled(!scanning && !deleting, egui::Button::new(format!("{}  Rescan", icons::ARROWS_CLOCKWISE))).clicked() {
                        self.start_scan(ctx);
                    }
                });
            });
            if !self.delete_errors.is_empty() {
                let title = format!("{} could not be deleted", self.delete_errors.len());
                egui::CollapsingHeader::new(egui::RichText::new(title).color(p.red)).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                        for e in &self.delete_errors {
                            ui.label(e);
                        }
                    });
                });
            }
        });

        // Each view remembers its own selection; fall back to the first entry when it is gone.
        let apps_view = self.view == View::Apps;
        if let Some(mut groups) = self.groups.take() {
            for (is_app, slot) in [(false, &mut self.selected), (true, &mut self.selected_app)] {
                if !groups.iter().any(|g| g.app == is_app && slot.as_deref() == Some(g.name.as_str())) {
                    *slot = groups.iter().find(|g| g.app == is_app).map(|g| g.name.clone());
                }
            }
            let side = egui::Frame::new().fill(p.side).inner_margin(egui::Margin::same(14));
            egui::Panel::left("groups").frame(side).resizable(false).exact_size(330.0).show(ui, |ui| {
                sidebar_ui(ui, &mut groups, self);
            });
            self.groups = Some(groups);
        }

        let mut open = None;
        let content = egui::Frame::new().fill(p.bg).inner_margin(egui::Margin::symmetric(24, 20));
        egui::CentralPanel::default().frame(content).show(ui, |ui| {
            let Some(groups) = &mut self.groups else {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Scanning your disk");
                    ui.weak(format!("{} found so far", human(self.seen.load(Ordering::Relaxed))));
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            };
            let key = if apps_view { &self.selected_app } else { &self.selected };
            let shown = groups.iter_mut().find(|g| g.app == apps_view && key.as_deref() == Some(g.name.as_str()));
            let Some(group) = shown else {
                ui.add_space(12.0);
                ui.label(if apps_view {
                    "No apps found in the Applications folders."
                } else {
                    "Nothing over 50 MB found that is known to be deletable."
                });
                return;
            };
            // No ticking or unticking while a delete is running on the old selection.
            open = ui.add_enabled_ui(!deleting, |ui| items_ui(ui, group, &self.home)).inner;
        });

        if self.confirming {
            let mut choice = None;
            let modal = egui::Modal::new(egui::Id::new("confirm_delete")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.heading(format!("Delete {count} items ({})?", human(bytes)));
                if risky > 0 {
                    ui.colored_label(
                        p.amber,
                        format!("{risky} of these do not come back: what is inside them is lost."),
                    );
                }
                ui.add_space(4.0);
                ui.label("Move to Trash can be undone, and frees the space once you empty the Trash.");
                ui.label("Delete permanently frees the space now and cannot be undone.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Move to Trash").clicked() {
                        choice = Some(Some(Mode::Trash));
                    }
                    let text = egui::RichText::new("Delete permanently").color(egui::Color32::WHITE);
                    if ui.add(egui::Button::new(text).fill(p.red)).clicked() {
                        choice = Some(Some(Mode::Permanent));
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(None);
                    }
                });
            });
            if modal.should_close() {
                choice = Some(None); // Escape or a click outside the box cancels
            }
            if let Some(mode) = choice {
                self.confirming = false;
                if let Some(mode) = mode {
                    self.start_delete(mode, ctx);
                }
            }
        }

        if deleting {
            let pr = self.progress.lock().unwrap();
            let (done, total) = pr.bytes;
            let fraction = if total == 0 { 1.0 } else { done as f32 / total as f32 };
            // No buttons and no closing: a delete cannot be stopped halfway safely.
            egui::Modal::new(egui::Id::new("delete_progress")).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.heading("Deleting…");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add(egui::Label::new(&pr.current).truncate());
                });
                ui.add_space(6.0);
                ui.add(egui::ProgressBar::new(fraction).show_percentage().fill(p.red));
                ui.horizontal(|ui| {
                    ui.weak(format!("{} of {} items", pr.items.0, pr.items.1));
                    ui.weak(format!("{} of {}", human(done), human(total)));
                    if pr.failed > 0 {
                        ui.colored_label(p.red, format!("{} failed", pr.failed));
                    }
                });
            });
        }

        if self.about {
            let modal = egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.vertical_centered(|ui| {
                    let icon = egui::Image::from_bytes("bytes://icon.png", include_bytes!("../docs/icon.png"));
                    ui.add(icon.fit_to_exact_size(egui::vec2(96.0, 96.0)));
                    ui.label(egui::RichText::new("ByteBroom").size(22.0).strong());
                    ui.weak(concat!("Version ", env!("CARGO_PKG_VERSION")));
                    ui.add_space(10.0);
                    ui.visuals_mut().hyperlink_color = p.green; // links stand out only in here
                    ui.hyperlink_to(format!("{}  github.com/p32929/bytebroom", icons::GITHUB_LOGO), REPO_URL);
                    ui.hyperlink_to(format!("{}  p32929.github.io", icons::GLOBE), "https://p32929.github.io/");
                    ui.add_space(10.0);
                    ui.button("Close").clicked()
                })
                .inner
            });
            if modal.inner || modal.should_close() {
                self.about = false;
            }
        }

        if let Some((path, is_dir)) = open {
            open_in_finder(&path, is_dir);
        }
    }
}

/// Scans the whole disk and returns what can be deleted.
fn scan_disk(home: &Path, seen: &AtomicU64) -> Vec<Group> {
    // On macOS "/" is a small read-only system volume and everything else (/Users,
    // /Library, /private, …) lives on a second data volume, so allow both.
    let dev = |p: &str| std::fs::metadata(p).map(|m| m.dev()).unwrap_or(0);
    let root = Path::new("/");
    let tree = scan(root, "/".into(), [dev("/"), dev("/System/Volumes/Data")], seen);
    let mut groups = rules::classify(&tree, root, home);
    // Second opinion for project folders: if git tracks files inside one, it is source, not junk.
    for g in groups.iter_mut().filter(|g| g.name == rules::PROJECT_GROUP) {
        g.items.retain(|f| {
            if f.needs_ignore {
                is_git_ignored(&f.path) // `dist`, `bin`…: only with proof that it is generated
            } else {
                !has_tracked_files(&f.path)
            }
        });
        g.total = g.items.iter().map(|f| f.size).sum();
    }
    groups.retain(|g| !g.items.is_empty());
    groups.extend(apps::find_apps(&tree, home));
    groups
}

/// True only when the folder is in a git repository whose ignore rules cover it.
fn is_git_ignored(dir: &Path) -> bool {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else { return false };
    Command::new("git")
        .arg("-C")
        .arg(parent)
        .args(["check-ignore", "-q", "--"])
        .arg(name)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// True when the folder is inside a git repository that tracks at least one file in it.
/// No git, or not a repository, counts as "not tracked": the marker-file rule still applies.
fn has_tracked_files(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "-z", "--", "."])
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty())
}

/// `bytebroom --print`: same scan and same list, written to the terminal instead of a window.
fn print_report() {
    let home = home_dir();
    for g in scan_disk(&home, &AtomicU64::new(0)) {
        let kind = if g.app { "app" } else if g.safe { "safe" } else { "look first" };
        println!("\n{:>9}  {} [{}]", human(g.total), g.name, kind);
        for f in &g.items {
            println!("{:>9}    {}", human(f.size), pretty(&f.path, &home));
        }
    }
}

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--print") {
        print_report();
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ByteBroom",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bytebroom_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sizes_roll_up_and_sort_largest_first() {
        let dir = temp("scan");
        std::fs::create_dir_all(dir.join("big")).unwrap();
        std::fs::create_dir_all(dir.join("small")).unwrap();
        std::fs::write(dir.join("big/blob"), vec![1u8; 3 << 20]).unwrap();
        std::fs::write(dir.join("big/Cargo.toml"), b"").unwrap();
        std::fs::write(dir.join("small/note"), b"hi").unwrap();

        let dev = std::fs::metadata(&dir).unwrap().dev();
        let seen = AtomicU64::new(0);
        let root = scan(&dir, "root".into(), [dev, dev], &seen);
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(root.children[0].name, "big");
        assert_eq!(root.children[1].name, "small");
        assert_eq!(root.children[0].children[0].name, "blob");
        assert_eq!(root.children[0].markers, rules::CARGO);
        assert!(root.children[1].children.is_empty(), "tiny files get no row");
        assert!(root.children[0].size >= 3 << 20);
        assert_eq!(root.size, root.children.iter().map(|c| c.size).sum::<u64>());
        assert_eq!(root.size, seen.load(Ordering::Relaxed));
    }

    #[test]
    fn permanent_delete_removes_only_the_target() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("delete");
        let home = Path::new("/Users/nobody");
        std::fs::create_dir_all(dir.join("junk/locked")).unwrap();
        std::fs::write(dir.join("junk/locked/f"), b"x").unwrap();
        std::fs::write(dir.join("keep"), b"x").unwrap();
        std::fs::write(dir.join("precious"), b"x").unwrap();
        std::os::unix::fs::symlink(dir.join("precious"), dir.join("link")).unwrap();
        // Read-only folder, like the Go module cache: needs the chmod-and-retry path.
        std::fs::set_permissions(dir.join("junk/locked"), std::fs::Permissions::from_mode(0o555)).unwrap();

        remove(&dir.join("junk"), Mode::Permanent, home).unwrap();
        remove(&dir.join("link"), Mode::Permanent, home).unwrap();

        assert!(!dir.join("junk").exists());
        assert!(dir.join("keep").exists(), "siblings are untouched");
        assert!(dir.join("precious").exists(), "deleting a link leaves its target");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_home_and_top_level_folders() {
        let home = Path::new("/Users/me");
        for p in ["/", "/Users", "/Users/me", "/Library/Caches", "/private/tmp", "relative/a/b/c"] {
            assert!(!is_deletable(Path::new(p), home), "{p} must be refused");
            assert!(remove(Path::new(p), Mode::Permanent, home).is_err());
        }
        assert!(is_deletable(Path::new("/Users/me/.gradle/caches"), home));
        assert!(is_deletable(Path::new("/private/tmp/build"), home));
        assert!(is_deletable(Path::new("/Applications/Some App.app"), home));
        assert!(!is_deletable(Path::new("/Applications"), home));
        assert!(!is_deletable(Path::new("/Library/Thing.app"), home));
    }

    #[test]
    fn git_tracked_folders_are_recognised() {
        let dir = temp("git");
        let git = |args: &[&str]| Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
        git(&["init", "-q"]);
        std::fs::create_dir_all(dir.join("build")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("build/real_source.txt"), b"x").unwrap();
        std::fs::write(dir.join("node_modules/junk.js"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join(".gitignore"), b"dist/\n").unwrap();
        git(&["add", "build"]);

        assert!(is_git_ignored(&dir.join("dist")), "listed in .gitignore");
        assert!(!is_git_ignored(&dir.join("out")), "not ignored, so not provably generated");
        assert!(!is_git_ignored(Path::new("/private/tmp/nope")), "outside a repo proves nothing");

        assert!(has_tracked_files(&dir.join("build")), "a tracked folder is source");
        assert!(!has_tracked_files(&dir.join("node_modules")), "an untracked folder is junk");
        assert!(!has_tracked_files(Path::new("/private/tmp")), "outside a repo is not tracked");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Puts one tiny file in the real Trash, so it only runs when asked for:
    // cargo test --release -- --ignored
    #[test]
    #[ignore]
    fn trash_mode_moves_the_file_out() {
        let dir = temp("trash");
        let file = dir.join("bytebroom_trash_test.txt");
        std::fs::write(&file, b"x").unwrap();
        remove(&file, Mode::Trash, &home_dir()).unwrap();
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
