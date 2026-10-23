//! Progress window of one download (IDM's "download status" dialog): opens
//! after "Baixar agora" and on double click in the list. When the download
//! finishes it becomes "Download concluído", with "Abrir" and "Abrir pasta".

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use ldm_ipc::{DownloadInfo, Request, State};

use crate::format;
use crate::window::{open_file_then, open_folder, send};

pub struct ProgressWindow {
    window: gtk::Window,
    name: gtk::Label,
    bar: gtk::ProgressBar,
    status: gtk::Label,
    downloaded: gtk::Label,
    speed: gtk::Label,
    remaining: gtk::Label,
    resumable: gtk::Label,
    dir: gtk::Label,
    error: gtk::Label,
    pause: gtk::Button,
    open: gtk::Button,
    open_folder: gtk::Button,
    /// This download's own speed limit (IDM's "Speed Limiter" in the same window).
    limit_on: gtk::CheckButton,
    limit: gtk::SpinButton,
    limit_row: gtk::Box,
    /// Set while the widgets follow the daemon, so they do not send it back.
    syncing: std::cell::Cell<bool>,
    info: RefCell<DownloadInfo>,
    on_closed: RefCell<Option<Box<dyn Fn()>>>,
}

fn value_label() -> gtk::Label {
    gtk::Label::builder().xalign(0.0).hexpand(true).ellipsize(pango::EllipsizeMode::Middle).build()
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(1.0).build();
    label.add_css_class("dim-label");
    label
}

/// Window title, like IDM's: "45% arquivo.mp4".
pub fn title(info: &DownloadInfo) -> String {
    match info.state {
        State::Completed => "Download concluído".into(),
        State::Failed => format!("Erro: {}", info.name),
        _ if info.total.is_some() => format!("{} {}", format::progress_text(info), info.name),
        _ => info.name.clone(),
    }
}

impl ProgressWindow {
    pub fn new(app: &gtk::Application, parent: Option<&gtk::ApplicationWindow>, info: &DownloadInfo) -> Rc<Self> {
        let window = gtk::Window::builder()
            .application(app)
            .title(title(info))
            .default_width(520)
            .resizable(false)
            .build();
        if let Some(parent) = parent {
            window.set_transient_for(Some(parent));
        }

        let name = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .max_width_chars(50)
            .build();
        name.add_css_class("title-4");
        let url = gtk::Label::builder()
            .label(&info.url)
            .tooltip_text(&info.url)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .max_width_chars(60)
            .build();
        url.add_css_class("dim-label");
        url.add_css_class("caption");
        let bar = gtk::ProgressBar::builder().show_text(true).build();

        let (status, downloaded, speed, remaining, resumable, dir) =
            (value_label(), value_label(), value_label(), value_label(), value_label(), value_label());
        let grid = gtk::Grid::builder().column_spacing(12).row_spacing(6).build();
        for (row, (text, label)) in [
            ("Estado", &status),
            ("Baixado", &downloaded),
            ("Velocidade", &speed),
            ("Restante", &remaining),
            ("Retomável", &resumable),
            ("Salvar em", &dir),
        ]
        .into_iter()
        .enumerate()
        {
            grid.attach(&caption(text), 0, row as i32, 1, 1);
            grid.attach(label, 1, row as i32, 1, 1);
        }
        let limit_on = gtk::CheckButton::with_label("No máximo");
        let limit = gtk::SpinButton::with_range(1.0, 1_000_000.0, 64.0);
        limit.set_value(512.0);
        let limit_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        limit_row.append(&limit_on);
        limit_row.append(&limit);
        limit_row.append(&gtk::Label::new(Some("KB/s")));
        let limit_caption = caption("Limite");
        grid.attach(&limit_caption, 0, 6, 1, 1);
        grid.attach(&limit_row, 1, 6, 1, 1);
        limit_row.bind_property("visible", &limit_caption, "visible").sync_create().build();

        let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).selectable(true).build();
        error.add_css_class("error");

        let folder_button = gtk::Button::with_label("Abrir pasta");
        let open = gtk::Button::with_label("Abrir");
        open.add_css_class("suggested-action");
        let pause = gtk::Button::with_label("Pausar");
        let close = gtk::Button::with_label("Fechar");
        let buttons = gtk::Box::builder().spacing(8).margin_top(6).build();
        let spacer = gtk::Box::builder().hexpand(true).build();
        buttons.append(&folder_button);
        buttons.append(&spacer);
        buttons.append(&pause);
        buttons.append(&close);
        buttons.append(&open);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        content.append(&name);
        content.append(&url);
        content.append(&bar);
        content.append(&grid);
        content.append(&error);
        content.append(&buttons);
        window.set_child(Some(&content));

        let this = Rc::new(Self {
            window,
            name,
            bar,
            status,
            downloaded,
            speed,
            remaining,
            resumable,
            dir,
            error,
            pause: pause.clone(),
            open: open.clone(),
            open_folder: folder_button.clone(),
            limit_on,
            limit,
            limit_row,
            syncing: std::cell::Cell::new(false),
            info: RefCell::new(info.clone()),
            on_closed: RefCell::default(),
        });
        this.update(info);

        // The limit applies at once: on ticking the box or changing the number.
        let send_limit = {
            let w = Rc::downgrade(&this);
            move || {
                let Some(this) = w.upgrade() else { return };
                if this.syncing.get() {
                    return;
                }
                let limit = match this.limit_on.is_active() {
                    true => this.limit.value() as u64 * 1024,
                    false => 0,
                };
                this.limit.set_sensitive(this.limit_on.is_active());
                let id = this.info.borrow().id;
                send(Request::SetSpeedLimit { id, limit }, Some(this.window.clone()));
            }
        };
        let s = send_limit.clone();
        this.limit_on.connect_toggled(move |_| s());
        this.limit.connect_value_changed(move |_| send_limit());

        let w = Rc::downgrade(&this);
        pause.connect_clicked(move |_| {
            let Some(this) = w.upgrade() else { return };
            let info = this.info.borrow().clone();
            let request = match info.state {
                State::Running | State::Queued => Request::Pause { id: info.id },
                _ => Request::Resume { id: info.id },
            };
            send(request, Some(this.window.clone()));
        });
        let w = Rc::downgrade(&this);
        open.connect_clicked(move |button| {
            let Some(this) = w.upgrade() else { return };
            let Some(path) = this.info.borrow().path.clone() else {
                this.window.close();
                return;
            };
            button.set_sensitive(false);
            let (window, button) = (this.window.clone(), button.clone());
            // Close only once the app was launched (see open_file_then).
            open_file_then(&this.window, path, move || window.close());
            glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || button.set_sensitive(true));
        });
        let w = Rc::downgrade(&this);
        folder_button.connect_clicked(move |_| {
            if let Some(this) = w.upgrade() {
                open_folder(&this.window, &this.info.borrow());
            }
        });
        let win = this.window.clone();
        close.connect_clicked(move |_| win.close());

        let keys = gtk::EventControllerKey::new();
        let win = this.window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                win.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        this.window.add_controller(keys);

        let w = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(callback) = w.upgrade().and_then(|this| this.on_closed.take()) {
                callback();
            }
            glib::Propagation::Proceed
        });
        this
    }

    pub fn connect_closed(&self, callback: impl Fn() + 'static) {
        self.on_closed.replace(Some(Box::new(callback)));
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn close(&self) {
        self.window.close();
    }

    pub fn update(&self, info: &DownloadInfo) {
        let finished_now = info.state == State::Completed && self.info.borrow().state != State::Completed;
        self.info.replace(info.clone());
        self.window.set_title(Some(&title(info)));
        self.name.set_label(&info.name);
        self.name.set_tooltip_text(Some(&info.name));
        self.bar.set_fraction(format::fraction(info));
        self.bar.set_text(Some(&format::progress_text(info)));
        self.status.set_label(&format::status(info));
        self.downloaded.set_label(&format::downloaded(info));
        self.speed.set_label(&or_dash(format::speed(info)));
        self.remaining.set_label(&or_dash(format::remaining(info)));
        self.resumable.set_label(if info.resumable { "Sim (pode pausar e continuar depois)" } else { "Não" });
        let dir = info
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.display().to_string()))
            .or_else(|| info.dir.as_ref().map(|d| d.display().to_string()))
            .unwrap_or_default();
        self.dir.set_label(&dir);
        self.dir.set_tooltip_text(Some(&dir));

        let failed = info.state == State::Failed;
        self.error.set_label(info.error.as_deref().unwrap_or_default());
        self.error.set_visible(failed && info.error.is_some());

        let done = info.state == State::Completed;
        // The limit follows the daemon, unless the user is typing a new one.
        self.limit_row.set_visible(!done);
        self.syncing.set(true);
        self.limit_on.set_active(info.speed_limit > 0);
        self.limit.set_sensitive(info.speed_limit > 0);
        if info.speed_limit > 0 && !self.limit.has_focus() {
            self.limit.set_value((info.speed_limit / 1024).max(1) as f64);
        }
        self.syncing.set(false);
        self.open.set_visible(done);
        self.open_folder.set_visible(done || info.path.is_some());
        self.pause.set_visible(!done && info.state != State::Pending);
        self.pause.set_label(match info.state {
            State::Running | State::Queued => "Pausar",
            State::Failed => "Tentar de novo",
            _ => "Retomar",
        });
        if finished_now {
            self.window.set_default_widget(Some(&self.open));
            self.open.grab_focus();
        }
    }
}

fn or_dash(text: String) -> String {
    if text.is_empty() {
        "—".into()
    } else {
        text
    }
}
