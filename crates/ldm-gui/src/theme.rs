//! The look: dark, with an amber accent and cyan for speeds, monospaced
//! numbers, and small CSS animations (the live dot, the cards fading in,
//! the glow of the add button). Applied to every window of the app.

use gtk::{gdk, prelude::*};

/// Cairo colors (RGB, 0–1) of the drawn parts: the speed graph and the cards.
pub const ACCENT: (f64, f64, f64) = (0.961, 0.620, 0.043); // #f59e0b
pub const CYAN: (f64, f64, f64) = (0.133, 0.827, 0.933); // #22d3ee
pub const GREEN: (f64, f64, f64) = (0.204, 0.827, 0.600); // #34d399
pub const RED: (f64, f64, f64) = (0.973, 0.443, 0.443); // #f87171
pub const MUTED: (f64, f64, f64) = (0.545, 0.576, 0.639); // #8b93a3
pub const TRACK: (f64, f64, f64) = (0.122, 0.149, 0.200); // #1f2633

const CSS: &str = r#"
window.ldm, window.ldm .ldm-main {
  background-color: #0f131a;
  color: #e6e9ef;
}
window.ldm headerbar {
  background: #141922;
  box-shadow: none;
  border-bottom: 1px solid #232a36;
  color: #e6e9ef;
}
window.ldm headerbar entry, window.ldm searchbar entry {
  background: #1b212c;
  border: 1px solid #2a3240;
  border-radius: 9px;
  color: #e6e9ef;
}
.ldm-logo {
  background: #f59e0b;
  color: #111;
  border-radius: 8px;
  min-width: 26px;
  min-height: 26px;
}
.ldm-title { font-weight: 800; letter-spacing: 1px; }

.ldm-sidebar, .ldm-sidebar list {
  background: #11161e;
  color: #c4cad6;
}
.ldm-sidebar { border-right: 1px solid #232a36; }
.ldm-sidebar row {
  border-radius: 7px;
  margin: 1px 8px;
  transition: background 150ms ease-out, box-shadow 150ms ease-out;
}
.ldm-sidebar row:hover { background: #1c2230; }
.ldm-sidebar row:selected {
  background: #1c2230;
  color: #f5f7fa;
  box-shadow: inset 3px 0 #f59e0b;
}
.ldm-heading {
  font-size: 0.78em;
  font-weight: 700;
  letter-spacing: 1px;
  color: #8b93a3;
}
.ldm-count, .ldm-mono {
  font-family: "JetBrains Mono", "Source Code Pro", monospace;
  font-feature-settings: "tnum";
}
.ldm-count { font-size: 0.85em; color: #8b93a3; }

.ldm-speed {
  background: #11161e;
  border-bottom: 1px solid #232a36;
}
.ldm-live {
  font-size: 0.78em;
  font-weight: 700;
  letter-spacing: 1px;
  color: #8b93a3;
}
.ldm-live-dot {
  background: #22d3ee;
  border-radius: 50%;
  min-width: 7px;
  min-height: 7px;
  animation: ldm-blink 1.2s ease-in-out infinite;
}
@keyframes ldm-blink {
  0% { opacity: 1; }
  50% { opacity: 0.25; }
  100% { opacity: 1; }
}
.ldm-speed-value { font-size: 28px; font-weight: 700; color: #f5f7fa; }
.ldm-speed-unit { font-size: 15px; color: #8b93a3; }
.ldm-dim { color: #8b93a3; }
.ldm-cyan { color: #22d3ee; }

.ldm-chip {
  border-radius: 15px;
  padding: 2px 12px;
  min-height: 26px;
  background: #151b25;
  border: 1px solid #2a3240;
  color: #c4cad6;
  box-shadow: none;
  transition: background 150ms ease-out, border-color 150ms ease-out;
}
.ldm-chip:hover { border-color: #4b5567; }
.ldm-chip:checked {
  background: #2a2112;
  border-color: #f59e0b;
  color: #fbbf24;
}

gridview.ldm-cards { background: transparent; }
gridview.ldm-cards > child {
  padding: 7px;
  background: transparent;
  border-radius: 18px;
}
gridview.ldm-cards > child:selected { background: transparent; }
.ldm-card {
  background: #151b25;
  border: 1px solid #232a36;
  border-radius: 16px;
  padding: 14px;
  transition: background 200ms ease-out, border-color 200ms ease-out;
  animation: ldm-card-in 450ms ease-out;
}
.ldm-card:hover { background: #18202b; border-color: #3a4456; }
gridview.ldm-cards > child:selected .ldm-card { border-color: #f59e0b; }
@keyframes ldm-card-in {
  from { opacity: 0; }
  to { opacity: 1; }
}
.ldm-card-name { font-weight: 600; font-size: 1.05em; color: #f5f7fa; }
.ldm-ext {
  font-size: 0.75em;
  padding: 2px 7px;
  border-radius: 5px;
  background: #1f2633;
  color: #93c5fd;
}
.ldm-ring-text { font-weight: 700; font-size: 0.95em; color: #f5f7fa; }
.ldm-state-dot { border-radius: 50%; min-width: 7px; min-height: 7px; background: #8b93a3; }
.ldm-state-dot.running { background: #f59e0b; }
.ldm-state-dot.done { background: #34d399; }
.ldm-state-dot.failed { background: #f87171; }
.ldm-status-failed { color: #f87171; }
.ldm-status-done { color: #34d399; }

columnview.ldm-list, columnview.ldm-list listview {
  background: #0f131a;
  color: #e6e9ef;
}

.ldm-fab {
  border-radius: 25px;
  padding: 10px 20px 10px 16px;
  background: #f59e0b;
  color: #111;
  font-weight: 700;
  box-shadow: 0 8px 22px alpha(#f59e0b, 0.25);
  animation: ldm-glow 2.4s ease-in-out infinite;
}
.ldm-fab:hover { background: #fbbf24; }
@keyframes ldm-glow {
  0% { box-shadow: 0 8px 22px alpha(#f59e0b, 0.25); }
  50% { box-shadow: 0 8px 34px alpha(#f59e0b, 0.45); }
  100% { box-shadow: 0 8px 22px alpha(#f59e0b, 0.25); }
}

.ldm-statusbar {
  color: #8b93a3;
  font-size: 0.9em;
  border-top: 1px solid #232a36;
  background: #0f131a;
}

button.suggested-action {
  background: #f59e0b;
  color: #111;
}
button.suggested-action:hover { background: #fbbf24; }
progressbar > trough > progress { background: #f59e0b; }
"#;

/// Dark theme and the app's stylesheet, for every window.
pub fn load() {
    let Some(display) = gdk::Display::default() else { return };
    gtk::Settings::for_display(&display).set_gtk_application_prefer_dark_theme(true);
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
}

/// Sets a cairo source color.
pub fn set_color(cr: &gtk::cairo::Context, (r, g, b): (f64, f64, f64), alpha: f64) {
    cr.set_source_rgba(r, g, b, alpha);
}
