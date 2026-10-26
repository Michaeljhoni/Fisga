//! The downloads as cards: a ring with the progress (it eases to each new
//! value), the badge, name and size, the file map (how much of each eighth
//! of the file is downloaded, with a sheen running up while it downloads),
//! and the state with the speed. Same model, selection, row menu and
//! double click as the list.

use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use ldm_ipc::{DownloadInfo, State, MAP_PARTS};

use crate::format;
use crate::item::DownloadItem;
use crate::theme::{self, set_color};

pub struct Cards {
    pub root: gtk::ScrolledWindow,
    /// The row menu, opened on a card.
    pub popover: gtk::PopoverMenu,
}

/// The widgets of one card and what its drawings show.
struct Card {
    root: gtk::Box,
    ring: gtk::DrawingArea,
    ring_text: gtk::Label,
    badge: gtk::Label,
    name: gtk::Label,
    size: gtk::Label,
    map: gtk::DrawingArea,
    dot: gtk::Box,
    status: gtk::Label,
    speed: gtk::Label,
    /// The ring's drawn fraction, easing to `target`.
    shown: Cell<f64>,
    target: Cell<f64>,
    color: Cell<(f64, f64, f64)>,
    parts: Cell<[f64; MAP_PARTS]>,
    running: Cell<bool>,
    /// The item shown and its notify handlers, dropped on unbind.
    bound: RefCell<Option<(DownloadItem, Vec<glib::SignalHandlerId>)>>,
}

fn state_style(info: &DownloadInfo) -> (&'static str, (f64, f64, f64)) {
    let flagged = info.error.is_some() || info.checksum_matches() == Some(false);
    match info.state {
        State::Running => ("running", theme::ACCENT),
        State::Completed if flagged => ("failed", theme::RED),
        State::Completed => ("done", theme::GREEN),
        State::Failed => ("failed", theme::RED),
        _ => ("idle", theme::MUTED),
    }
}

impl Card {
    fn new() -> Rc<Self> {
        let ring = gtk::DrawingArea::builder().content_width(62).content_height(62).build();
        let ring_text = gtk::Label::new(None);
        ring_text.add_css_class("ldm-ring-text");
        ring_text.add_css_class("ldm-mono");
        let ring_box = gtk::Overlay::builder().child(&ring).valign(gtk::Align::Center).build();
        ring_box.add_overlay(&ring_text);

        let badge = gtk::Label::builder().halign(gtk::Align::Start).build();
        badge.add_css_class("ldm-ext");
        badge.add_css_class("ldm-mono");
        let name = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).max_width_chars(28).build();
        name.add_css_class("ldm-card-name");
        let size = gtk::Label::builder().xalign(0.0).build();
        size.add_css_class("ldm-dim");
        size.add_css_class("ldm-mono");
        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        info.append(&badge);
        info.append(&name);
        info.append(&size);
        // Every part at its own height: a card is as tall as its content.
        let top = gtk::Box::builder().spacing(14).valign(gtk::Align::Start).vexpand(false).build();
        top.append(&ring_box);
        top.append(&info);

        let map = gtk::DrawingArea::builder().content_height(28).hexpand(true).valign(gtk::Align::Start).build();

        let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
        dot.add_css_class("ldm-state-dot");
        let status =
            gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::End).max_width_chars(30).build();
        let speed = gtk::Label::new(None);
        speed.add_css_class("ldm-cyan");
        speed.add_css_class("ldm-mono");
        let bottom = gtk::Box::builder().spacing(8).valign(gtk::Align::Start).vexpand(false).build();
        bottom.append(&dot);
        bottom.append(&status);
        bottom.append(&speed);

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .width_request(230)
            .valign(gtk::Align::Start)
            .build();
        root.add_css_class("ldm-card");
        root.append(&top);
        root.append(&map);
        root.append(&bottom);

        let card = Rc::new(Self {
            root,
            ring,
            ring_text,
            badge,
            name,
            size,
            map,
            dot,
            status,
            speed,
            shown: Cell::new(0.0),
            target: Cell::new(0.0),
            color: Cell::new(theme::MUTED),
            parts: Cell::new([0.0; MAP_PARTS]),
            running: Cell::new(false),
            bound: RefCell::default(),
        });
        // The drawings own the card: they go with the widgets.
        let c = card.clone();
        card.ring.set_draw_func(move |_, cr, w, h| draw_ring(cr, f64::from(w), f64::from(h), c.shown.get(), c.color.get()));
        let c = card.clone();
        card.map.set_draw_func(move |_, cr, w, h| {
            draw_map(cr, f64::from(w), f64::from(h), &c.parts.get(), c.color.get(), c.running.get())
        });
        let weak = Rc::downgrade(&card);
        card.ring.add_tick_callback(move |_, _| {
            let Some(card) = weak.upgrade() else { return glib::ControlFlow::Break };
            let (shown, target) = (card.shown.get(), card.target.get());
            if (target - shown).abs() > 0.002 {
                card.shown.set(shown + (target - shown) * 0.12);
                card.ring.queue_draw();
            } else if shown != target {
                card.shown.set(target);
                card.ring.queue_draw();
            }
            if card.running.get() {
                card.map.queue_draw();
            }
            glib::ControlFlow::Continue
        });
        card
    }

    fn fill(&self, info: &DownloadInfo) {
        self.name.set_text(&info.name);
        self.name.set_tooltip_text(Some(&info.name));
        self.badge.set_text(&format::badge(info));
        self.size.set_text(&format::downloaded(info));
        self.status.set_text(&format::status(info));
        self.speed.set_text(&format::speed(info));
        self.ring_text.set_text(&format::ring_text(info));
        let (class, color) = state_style(info);
        for name in ["running", "done", "failed", "idle"] {
            self.dot.remove_css_class(name);
        }
        self.dot.add_css_class(class);
        self.status.remove_css_class("ldm-status-failed");
        self.status.remove_css_class("ldm-status-done");
        match class {
            "failed" => self.status.add_css_class("ldm-status-failed"),
            "done" => self.status.add_css_class("ldm-status-done"),
            _ => {}
        }
        self.color.set(color);
        self.target.set(format::fraction(info));
        self.parts.set(format::file_map(info));
        self.running.set(info.state == State::Running);
        self.ring.queue_draw();
        self.map.queue_draw();
    }

    /// Shows `item` and follows its updates.
    fn bind(self: &Rc<Self>, item: &DownloadItem) {
        self.unbind();
        self.fill(&item.info());
        // The ring grows from zero when a card appears.
        self.shown.set(0.0);
        let handlers = ["status", "progress-text", "speed", "name"]
            .into_iter()
            .map(|property| {
                let weak = Rc::downgrade(self);
                item.connect_notify_local(Some(property), move |item, _| {
                    if let Some(card) = weak.upgrade() {
                        card.fill(&item.info());
                    }
                })
            })
            .collect();
        self.bound.replace(Some((item.clone(), handlers)));
    }

    fn unbind(&self) {
        if let Some((item, handlers)) = self.bound.take() {
            for handler in handlers {
                item.disconnect(handler);
            }
        }
    }
}

fn draw_ring(cr: &gtk::cairo::Context, width: f64, height: f64, fraction: f64, color: (f64, f64, f64)) {
    let (cx, cy) = (width / 2.0, height / 2.0);
    let radius = width.min(height) / 2.0 - 4.0;
    cr.set_line_width(6.0);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    set_color(cr, theme::TRACK, 1.0);
    cr.arc(cx, cy, radius, 0.0, 2.0 * PI);
    let _ = cr.stroke();
    if fraction > 0.003 {
        set_color(cr, color, 1.0);
        cr.arc(cx, cy, radius, -PI / 2.0, -PI / 2.0 + 2.0 * PI * fraction.min(1.0));
        let _ = cr.stroke();
    }
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -PI / 2.0, 0.0);
    cr.arc(x + width - radius, y + height - radius, radius, 0.0, PI / 2.0);
    cr.arc(x + radius, y + height - radius, radius, PI / 2.0, PI);
    cr.arc(x + radius, y + radius, radius, PI, 1.5 * PI);
    cr.close_path();
}

fn draw_map(cr: &gtk::cairo::Context, width: f64, height: f64, parts: &[f64; MAP_PARTS], color: (f64, f64, f64), running: bool) {
    let gap = 3.0;
    let bar = (width - gap * (MAP_PARTS - 1) as f64) / MAP_PARTS as f64;
    // The sheen's place, from the clock: one pass every 1.6 s.
    let now = glib::monotonic_time() as f64 / 1_000_000.0;
    for (i, part) in parts.iter().enumerate() {
        let x = i as f64 * (bar + gap);
        set_color(cr, theme::TRACK, 1.0);
        rounded(cr, x, 0.0, bar, height, 3.0);
        let _ = cr.fill();
        // Something always shows, so an empty stretch is told from no stretch.
        let filled = (part * height).max(if *part > 0.0 { 2.0 } else { 0.0 });
        if filled <= 0.0 {
            continue;
        }
        set_color(cr, color, 1.0);
        rounded(cr, x, height - filled, bar, filled, 3.0);
        let _ = cr.fill();
        if running && *part < 1.0 {
            let phase = (now / 1.6 + i as f64 * 0.07).fract();
            let band = height * 0.4;
            let y = height - phase * (height + band);
            cr.save().ok();
            rounded(cr, x, height - filled, bar, filled, 3.0);
            cr.clip();
            cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
            cr.rectangle(x, y, bar, band);
            let _ = cr.fill();
            cr.restore().ok();
        }
    }
}

/// The card grid over `selection`; `on_activate` gets a double-clicked card.
pub fn new(selection: &gtk::SingleSelection, menu: &gio::Menu, on_activate: impl Fn(&DownloadItem) + 'static) -> Cards {
    let factory = gtk::SignalListItemFactory::new();
    let grid = gtk::GridView::builder()
        .model(selection)
        .factory(&factory)
        .min_columns(1)
        .max_columns(4)
        .build();
    grid.add_css_class("ldm-cards");
    let popover = gtk::PopoverMenu::builder().menu_model(menu).has_arrow(false).build();
    popover.set_halign(gtk::Align::Start);
    popover.set_parent(&grid);
    let p = popover.clone();
    grid.connect_destroy(move |_| p.unparent());

    let cards: Rc<RefCell<Vec<(glib::WeakRef<gtk::ListItem>, Rc<Card>)>>> = Rc::default();
    let (c, s, g, pop) = (cards.clone(), selection.clone(), grid.downgrade(), popover.clone());
    factory.connect_setup(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        let card = Card::new();
        list_item.set_child(Some(&card.root));
        // Right click: select the card and open the row menu there.
        let gesture = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
        let (item, root, s, g, pop) = (list_item.downgrade(), card.root.downgrade(), s.clone(), g.clone(), pop.clone());
        gesture.connect_pressed(move |gesture, _, x, y| {
            let (Some(item), Some(root), Some(grid)) = (item.upgrade(), root.upgrade(), g.upgrade()) else { return };
            gesture.set_state(gtk::EventSequenceState::Claimed);
            s.set_selected(item.position());
            let (x, y) = root.translate_coordinates(&grid, x, y).unwrap_or((x, y));
            pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.popup();
        });
        card.root.add_controller(gesture);
        let mut cards = c.borrow_mut();
        cards.retain(|(item, _)| item.upgrade().is_some());
        cards.push((list_item.downgrade(), card));
    });
    let find = move |list_item: &gtk::ListItem| {
        cards.borrow().iter().find(|(item, _)| item.upgrade().as_ref() == Some(list_item)).map(|(_, card)| card.clone())
    };
    let find = Rc::new(find);
    let f = find.clone();
    factory.connect_bind(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        if let (Some(card), Some(item)) = ((*f)(list_item), list_item.item().and_downcast::<DownloadItem>()) {
            card.bind(&item);
        }
    });
    factory.connect_unbind(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        if let Some(card) = (*find)(list_item) {
            card.unbind();
        }
    });

    grid.connect_activate(move |grid, position| {
        if let Some(item) = grid.model().and_then(|model| model.item(position)).and_downcast::<DownloadItem>() {
            on_activate(&item);
        }
    });
    let root = gtk::ScrolledWindow::builder()
        .child(&grid)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .hexpand(true)
        .build();
    Cards { root, popover }
}
