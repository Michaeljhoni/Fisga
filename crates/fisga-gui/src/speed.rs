//! The live speed graph at the top of the main window: the total speed of
//! the last minute as a line that scrolls smoothly, with the number and
//! what is going on drawn over it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::format;
use crate::theme;

/// Samples kept, one a second.
const HISTORY: usize = 60;

struct Samples {
    values: VecDeque<f64>,
    /// When the newest one came: between samples the line slides left.
    at: Instant,
}

pub struct SpeedPanel {
    pub root: gtk::Overlay,
    value: gtk::Label,
    unit: gtk::Label,
    detail: gtk::Label,
    current: Rc<Cell<f64>>,
}

impl SpeedPanel {
    pub fn new() -> Self {
        let samples = Rc::new(RefCell::new(Samples { values: vec![0.0; HISTORY].into(), at: Instant::now() }));
        let current = Rc::new(Cell::new(0.0));
        let area = gtk::DrawingArea::builder().hexpand(true).content_height(150).build();
        let s = samples.clone();
        area.set_draw_func(move |_, cr, width, height| draw(cr, f64::from(width), f64::from(height), &s.borrow()));
        // Redrawn every frame while the graph has something to show, for the slide.
        let s = samples.clone();
        area.add_tick_callback(move |area, _| {
            if s.borrow().values.iter().any(|v| *v > 0.0) {
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
        let (s, c, weak) = (samples.clone(), current.clone(), area.downgrade());
        glib::timeout_add_local(Duration::from_secs(1), move || {
            if weak.upgrade().is_none() {
                return glib::ControlFlow::Break;
            }
            let mut samples = s.borrow_mut();
            samples.values.pop_front();
            samples.values.push_back(c.get());
            samples.at = Instant::now();
            glib::ControlFlow::Continue
        });

        let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
        dot.add_css_class("fisga-live-dot");
        let live = gtk::Label::new(Some("AO VIVO"));
        live.add_css_class("fisga-live");
        let live_row = gtk::Box::builder().spacing(6).build();
        live_row.append(&dot);
        live_row.append(&live);
        let value = gtk::Label::new(Some("0"));
        value.add_css_class("fisga-speed-value");
        value.add_css_class("fisga-mono");
        let unit = gtk::Label::new(Some("B/s"));
        unit.add_css_class("fisga-speed-unit");
        let value_row = gtk::Box::builder().spacing(6).build();
        value_row.append(&value);
        value_row.append(&unit);
        let detail = gtk::Label::builder().xalign(0.0).build();
        detail.add_css_class("fisga-dim");
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_top(16)
            .margin_start(24)
            .build();
        text.append(&live_row);
        text.append(&value_row);
        text.append(&detail);

        let root = gtk::Overlay::builder().child(&area).build();
        root.add_overlay(&text);
        root.add_css_class("fisga-speed");
        Self { root, value, unit, detail, current }
    }

    /// What is going on now: the total speed and the downloads behind it.
    pub fn set(&self, speed: f64, running: usize, connections: usize, left: Option<u64>) {
        self.current.set(speed);
        let text = format::bytes(speed as u64);
        let (number, unit) = text.split_once(' ').unwrap_or((&text, "B"));
        self.value.set_text(number);
        self.unit.set_text(&format!("{unit}/s"));
        let detail = match running {
            0 => "Nenhum download em andamento".to_string(),
            n => {
                let mut parts = vec![if n == 1 { "1 baixando".to_string() } else { format!("{n} baixando") }];
                match connections {
                    0 => {}
                    1 => parts.push("1 conexão".to_string()),
                    n => parts.push(format!("{n} conexões")),
                }
                if let Some(seconds) = left {
                    parts.push(format!("termina em ~{}", format::duration(seconds)));
                }
                parts.join(" · ")
            }
        };
        self.detail.set_text(&detail);
    }
}

fn draw(cr: &gtk::cairo::Context, width: f64, height: f64, samples: &Samples) {
    let values = &samples.values;
    // At least 1 MB/s of scale, so a trickle does not look like a full pipe.
    let top = values.iter().copied().fold(1024.0 * 1024.0, f64::max) * 1.25;
    let step = width / (HISTORY - 2) as f64;
    let shift = samples.at.elapsed().as_secs_f64().min(1.0) * step;
    let point = |i: usize, value: f64| (i as f64 * step - shift, height - 6.0 - value / top * (height - 46.0));

    cr.set_line_join(gtk::cairo::LineJoin::Round);
    let (x0, y0) = point(0, values[0]);
    cr.move_to(x0, y0);
    for (i, value) in values.iter().enumerate().skip(1) {
        let (x, y) = point(i, *value);
        cr.line_to(x, y);
    }
    let (last_x, _) = point(values.len() - 1, 0.0);
    // The area under the line, faint, then the line.
    let line = cr.copy_path();
    cr.line_to(last_x, height);
    cr.line_to(x0, height);
    cr.close_path();
    theme::set_color(cr, theme::CYAN, 0.10);
    let _ = cr.fill();
    if let Ok(line) = line {
        cr.append_path(&line);
        theme::set_color(cr, theme::CYAN, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
    }
}
