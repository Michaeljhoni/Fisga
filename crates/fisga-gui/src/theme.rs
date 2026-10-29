//! The look: dark or light, with an amber accent (or the holiday's, see
//! `festive`) and cyan for speeds, monospaced numbers, and small CSS
//! animations (the live dot, the cards fading in, the glow of the add
//! button). Applied to every window of the app. The stylesheet names its
//! colors (`@fisga_base`...): a theme is a palette for those names.

use std::cell::{Cell, RefCell};

use gtk::gdk;
use gtk::prelude::*;

use crate::festive::Season;

/// The colors of one theme, as CSS hex.
struct Palette {
    /// The main window's background, under the cards.
    base: &'static str,
    header: &'static str,
    sidebar: &'static str,
    /// Cards, dialogs, chips.
    surface: &'static str,
    surface_hover: &'static str,
    field: &'static str,
    hover: &'static str,
    /// Under the rings and the file map (also drawn with cairo).
    track: &'static str,
    border: &'static str,
    line: &'static str,
    line_hover: &'static str,
    text: &'static str,
    strong: &'static str,
    soft: &'static str,
    muted: &'static str,
    link: &'static str,
    cyan: &'static str,
    green: &'static str,
    red: &'static str,
    /// The accent as text (a checked chip), readable on this background.
    accent_text: &'static str,
}

const DARK: Palette = Palette {
    base: "#0f131a",
    header: "#141922",
    sidebar: "#11161e",
    surface: "#151b25",
    surface_hover: "#18202b",
    field: "#1b212c",
    hover: "#1c2230",
    track: "#1f2633",
    border: "#232a36",
    line: "#2a3240",
    line_hover: "#445064",
    text: "#e6e9ef",
    strong: "#f5f7fa",
    soft: "#c4cad6",
    muted: "#8b93a3",
    link: "#93c5fd",
    cyan: "#22d3ee",
    green: "#34d399",
    red: "#f87171",
    accent_text: "#fbbf24",
};

const LIGHT: Palette = Palette {
    base: "#eef1f5",
    header: "#ffffff",
    sidebar: "#f7f8fa",
    surface: "#ffffff",
    surface_hover: "#fbfcfd",
    field: "#ffffff",
    hover: "#e6eaf0",
    track: "#e2e7ee",
    border: "#dde2e9",
    line: "#cfd5de",
    line_hover: "#aab3c0",
    text: "#1f2633",
    strong: "#0f131a",
    soft: "#3d4655",
    muted: "#667085",
    link: "#1d4ed8",
    cyan: "#0891b2",
    green: "#059669",
    red: "#dc2626",
    accent_text: "#b45309",
};

const AMBER: &str = "#f59e0b";

const CSS: &str = r#"
/* Every window (Novo download, progress, Preferências...) in the same palette. */
window.background {
  background-color: @fisga_surface;
  color: @fisga_text;
}
window.background entry, window.background spinbutton {
  background-color: @fisga_field;
  border-color: @fisga_line;
}
window.background progressbar > trough { background-color: @fisga_border; }
window.background notebook > header, window.background notebook > stack { background-color: @fisga_surface; }
window.background notebook > header tab:checked { box-shadow: inset 0 -2px @fisga_accent; }
switch:checked { background-color: @fisga_accent; }
checkbutton check:checked { background-color: @fisga_accent; color: #111; }
popover > contents, popover.menu > contents {
  background-color: @fisga_field;
  color: @fisga_text;
}
window.fisga, window.fisga .fisga-main {
  background-color: @fisga_base;
  color: @fisga_text;
}
window.fisga headerbar {
  background: @fisga_header;
  box-shadow: none;
  border-bottom: 1px solid @fisga_border;
  color: @fisga_text;
}
window.fisga headerbar entry, window.fisga searchbar entry {
  background: @fisga_field;
  border: 1px solid @fisga_line;
  border-radius: 9px;
  color: @fisga_text;
}
.fisga-title { font-weight: 800; letter-spacing: 1px; }

.fisga-sidebar, .fisga-sidebar list {
  background: @fisga_sidebar;
  color: @fisga_soft;
}
.fisga-sidebar { border-right: 1px solid @fisga_border; }
.fisga-sidebar row {
  border-radius: 7px;
  margin: 1px 8px;
  transition: background 150ms ease-out, box-shadow 150ms ease-out;
}
.fisga-sidebar row:hover { background: @fisga_hover; }
.fisga-sidebar row:selected {
  background: @fisga_hover;
  color: @fisga_strong;
  box-shadow: inset 3px 0 @fisga_accent;
}
.fisga-heading {
  font-size: 0.78em;
  font-weight: 700;
  letter-spacing: 1px;
  color: @fisga_muted;
}
.fisga-count, .fisga-mono {
  font-family: "JetBrains Mono", "Source Code Pro", monospace;
  font-feature-settings: "tnum";
}
.fisga-count { font-size: 0.85em; color: @fisga_muted; }

.fisga-speed {
  background: @fisga_sidebar;
  border-bottom: 1px solid @fisga_border;
}
.fisga-live {
  font-size: 0.78em;
  font-weight: 700;
  letter-spacing: 1px;
  color: @fisga_muted;
}
.fisga-live-dot {
  background: @fisga_cyan;
  border-radius: 50%;
  min-width: 7px;
  min-height: 7px;
  animation: fisga-blink 1.2s ease-in-out infinite;
}
@keyframes fisga-blink {
  0% { opacity: 1; }
  50% { opacity: 0.25; }
  100% { opacity: 1; }
}
.fisga-speed-value { font-size: 28px; font-weight: 700; color: @fisga_strong; }
.fisga-speed-unit { font-size: 15px; color: @fisga_muted; }
.fisga-dim { color: @fisga_muted; }
.fisga-cyan { color: @fisga_cyan; }

.fisga-chip {
  border-radius: 15px;
  padding: 2px 12px;
  min-height: 26px;
  background: @fisga_surface;
  border: 1px solid @fisga_line;
  color: @fisga_soft;
  box-shadow: none;
  transition: background 150ms ease-out, border-color 150ms ease-out;
}
.fisga-chip:hover { border-color: @fisga_line_hover; }
.fisga-chip:checked {
  background: alpha(@fisga_accent, 0.14);
  border-color: @fisga_accent;
  color: @fisga_accent_text;
}

gridview.fisga-cards { background: transparent; }
gridview.fisga-cards > child {
  padding: 7px;
  background: transparent;
  border-radius: 18px;
}
gridview.fisga-cards > child:selected { background: transparent; }
/* The default theme spaces out every box in a grid (icon views): not in cards. */
gridview.fisga-cards > child box {
  margin: 0;
  border-spacing: 0;
}
.fisga-card {
  background: @fisga_surface;
  border: 1px solid @fisga_border;
  border-radius: 16px;
  padding: 14px;
  transition: background 200ms ease-out, border-color 200ms ease-out;
  animation: fisga-card-in 450ms ease-out;
}
.fisga-card:hover { background: @fisga_surface_hover; border-color: @fisga_line_hover; }
gridview.fisga-cards > child:selected .fisga-card { border-color: @fisga_accent; }
@keyframes fisga-card-in {
  from { opacity: 0; }
  to { opacity: 1; }
}
.fisga-card-name { font-weight: 600; font-size: 1.05em; color: @fisga_strong; }
.fisga-ext {
  font-size: 0.75em;
  padding: 2px 7px;
  border-radius: 5px;
  background: @fisga_track;
  color: @fisga_link;
}
.fisga-ring-text { font-weight: 700; font-size: 0.95em; color: @fisga_strong; }
.fisga-state-dot { border-radius: 50%; min-width: 7px; min-height: 7px; background: @fisga_muted; }
.fisga-state-dot.running { background: @fisga_accent; }
.fisga-state-dot.done { background: @fisga_green; }
.fisga-state-dot.failed { background: @fisga_red; }
.fisga-status-failed { color: @fisga_red; }
.fisga-status-done { color: @fisga_green; }

columnview.fisga-list, columnview.fisga-list listview {
  background: @fisga_base;
  color: @fisga_text;
}

.fisga-fab {
  border-radius: 25px;
  padding: 10px 20px 10px 16px;
  background: @fisga_accent;
  color: #111;
  font-weight: 700;
  box-shadow: none;
}
.fisga-fab:hover { background: shade(@fisga_accent, 1.1); }
.fisga-fab-glow {
  border-radius: 25px;
  box-shadow: 0 8px 30px alpha(@fisga_accent, 0.45);
  opacity: 0.55;
  animation: fisga-glow 2.4s ease-in-out infinite;
}
@keyframes fisga-glow {
  0% { opacity: 0.55; }
  50% { opacity: 1; }
  100% { opacity: 0.55; }
}

.fisga-statusbar {
  color: @fisga_muted;
  font-size: 0.9em;
  border-top: 1px solid @fisga_border;
  background: @fisga_base;
}

button.suggested-action {
  background: @fisga_accent;
  color: #111;
}
button.suggested-action:hover { background: shade(@fisga_accent, 1.1); }
progressbar > trough > progress { background: @fisga_accent; }
"#;

thread_local! {
    static LIGHT_THEME: Cell<bool> = const { Cell::new(false) };
    /// The holiday of these days (when the effects are on).
    static SEASON: Cell<Option<Season>> = const { Cell::new(None) };
    /// The holiday being tried from the preferences, for the effect's length.
    static PREVIEW: Cell<Option<Season>> = const { Cell::new(None) };
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

fn palette() -> &'static Palette {
    if is_light() {
        &LIGHT
    } else {
        &DARK
    }
}

pub fn is_light() -> bool {
    LIGHT_THEME.get()
}

/// The accent and the accent as text.
fn accent() -> (&'static str, &'static str) {
    match PREVIEW.get().or(SEASON.get()) {
        Some(season) => {
            let (accent, on_dark, on_light) = season.accent();
            (accent, if is_light() { on_light } else { on_dark })
        }
        None => (AMBER, palette().accent_text),
    }
}

fn stylesheet() -> String {
    let p = palette();
    let (accent, accent_text) = accent();
    let colors = [
        ("base", p.base),
        ("header", p.header),
        ("sidebar", p.sidebar),
        ("surface", p.surface),
        ("surface_hover", p.surface_hover),
        ("field", p.field),
        ("hover", p.hover),
        ("track", p.track),
        ("border", p.border),
        ("line", p.line),
        ("line_hover", p.line_hover),
        ("text", p.text),
        ("strong", p.strong),
        ("soft", p.soft),
        ("muted", p.muted),
        ("link", p.link),
        ("cyan", p.cyan),
        ("green", p.green),
        ("red", p.red),
        ("accent", accent),
        ("accent_text", accent_text),
    ];
    let mut css: String = colors.iter().map(|(name, value)| format!("@define-color fisga_{name} {value};\n")).collect();
    css.push_str(CSS);
    css
}

/// The app's stylesheet, for every window, in the dark theme until the
/// settings arrive.
pub fn load() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    PROVIDER.replace(Some(provider));
    apply();
}

/// The theme and the holiday of the settings; nothing to do when they are
/// the ones shown.
pub fn set_style(light: bool, season: Option<Season>) {
    if (light, season) != (is_light(), SEASON.get()) {
        LIGHT_THEME.set(light);
        SEASON.set(season);
        apply();
    }
}

/// Only the theme: picked in the preferences, before it is saved.
pub fn set_light(light: bool) {
    set_style(light, SEASON.get());
}

/// Shows a holiday's colors while its effect is tried; `None` goes back.
pub fn preview(season: Option<Season>) {
    if season != PREVIEW.get() {
        PREVIEW.set(season);
        apply();
    }
}

fn apply() {
    let Some(display) = gdk::Display::default() else { return };
    // The widgets left to GTK's theme (buttons, menus, scrollbars).
    gtk::Settings::for_display(&display).set_gtk_application_prefer_dark_theme(!is_light());
    PROVIDER.with_borrow(|provider| {
        if let Some(provider) = provider {
            provider.load_from_string(&stylesheet());
        }
    });
    // What cairo draws takes its colors at drawing time.
    for window in gtk::Window::list_toplevels() {
        redraw(&window);
    }
}

fn redraw(widget: &gtk::Widget) {
    if widget.is::<gtk::DrawingArea>() {
        widget.queue_draw();
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        redraw(&widget);
        child = widget.next_sibling();
    }
}

/// The colors drawn with cairo: the speed graph and the cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Accent,
    Cyan,
    Green,
    Red,
    Muted,
    Track,
}

/// `#rrggbb` → RGB, 0–1.
pub fn rgb(hex: &str) -> (f64, f64, f64) {
    let channel = |at: usize| hex.get(at..at + 2).and_then(|c| u8::from_str_radix(c, 16).ok()).unwrap_or(0) as f64 / 255.0;
    (channel(1), channel(3), channel(5))
}

/// Sets a cairo source color.
pub fn set_color(cr: &gtk::cairo::Context, ink: Ink, alpha: f64) {
    let p = palette();
    let hex = match ink {
        Ink::Accent => accent().0,
        Ink::Cyan => p.cyan,
        Ink::Green => p.green,
        Ink::Red => p.red,
        Ink::Muted => p.muted,
        Ink::Track => p.track,
    };
    let (r, g, b) = rgb(hex);
    cr.set_source_rgba(r, g, b, alpha);
}
