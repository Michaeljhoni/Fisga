//! "Novo download" window, shown for every pending download (IDM's "Download
//! File Info"): name, size and destination folder, with "Baixar agora",
//! "Adicionar à fila" and "Cancelar". Closing the window cancels.
//!
//! The title is always exactly `Novo download`, so compositors can match it
//! (niri: `window-rule { match app-id="io.ldm.Ldm" title="^Novo download$" ... }`).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use ldm_ipc::{DownloadInfo, Request, SourceKind};

use crate::format;
use crate::ipc;

pub const TITLE: &str = "Novo download";

pub struct ConfirmDialog {
    id: u64,
    window: gtk::Window,
    name: gtk::Entry,
    size: gtk::Label,
    dir_label: gtk::Label,
    error: gtk::Label,
    buttons: Vec<gtk::Button>,
    /// "Adicionar à fila" and "Baixar agora": disabled while the daemon reports an error.
    answers: Vec<gtk::Button>,
    /// Name last suggested by the daemon; the entry follows it until the user edits it.
    suggested_name: RefCell<String>,
    /// Folder shown; follows the daemon (category subfolder) until the user picks one.
    dir: RefCell<Option<PathBuf>>,
    user_dir: Cell<bool>,
    /// Answered (or closed by the daemon): closing the window must not cancel.
    decided: Cell<bool>,
    on_closed: RefCell<Option<Box<dyn Fn()>>>,
    /// Called after "Baixar agora" or "Adicionar à fila" succeeded.
    on_answered: RefCell<Option<Box<dyn Fn()>>>,
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(1.0).build();
    label.add_css_class("dim-label");
    label
}

fn size_text(info: &DownloadInfo) -> String {
    let size = format::size(info);
    match info.kind {
        _ if info.probing => "verificando…".into(),
        SourceKind::Page | SourceKind::Stream if info.total.is_none() => {
            "definido ao baixar (vídeo de site, via yt-dlp)".into()
        }
        SourceKind::Page | SourceKind::Stream => format!("≈ {size}"),
        _ if info.total.is_some() && !info.resumable => format!("{size} (o servidor não permite pausar e retomar)"),
        _ => size,
    }
}

impl ConfirmDialog {
    pub fn new(app: &gtk::Application, parent: Option<&gtk::ApplicationWindow>, info: &DownloadInfo) -> Rc<Self> {
        let window = gtk::Window::builder()
            .application(app)
            .title(TITLE)
            .default_width(580)
            .resizable(false)
            .build();
        if let Some(parent) = parent {
            window.set_transient_for(Some(parent));
        }

        let url = gtk::Label::builder()
            .label(&info.url)
            .tooltip_text(&info.url)
            .xalign(0.0)
            .selectable(true)
            .ellipsize(pango::EllipsizeMode::Middle)
            .max_width_chars(60)
            .hexpand(true)
            .build();
        let name = gtk::Entry::builder().text(&info.name).activates_default(true).hexpand(true).build();
        let dir_label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Start)
            .hexpand(true)
            .build();
        let change_dir = gtk::Button::with_label("Alterar…");
        let dir_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        dir_row.append(&dir_label);
        dir_row.append(&change_dir);
        let size = gtk::Label::builder().xalign(0.0).wrap(true).build();

        let grid = gtk::Grid::builder().column_spacing(12).row_spacing(10).build();
        for (row, (text, widget)) in [
            ("Endereço", url.upcast_ref::<gtk::Widget>()),
            ("Salvar como", name.upcast_ref()),
            ("Pasta", dir_row.upcast_ref()),
            ("Tamanho", size.upcast_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            grid.attach(&caption(text), 0, row as i32, 1, 1);
            grid.attach(widget, 1, row as i32, 1, 1);
        }

        let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
        error.add_css_class("error");

        let cancel = gtk::Button::with_label("Cancelar");
        let queue = gtk::Button::with_label("Adicionar à fila");
        let now = gtk::Button::with_label("Baixar agora");
        now.add_css_class("suggested-action");
        let button_row = gtk::Box::builder().spacing(8).halign(gtk::Align::End).margin_top(6).build();
        button_row.append(&cancel);
        button_row.append(&queue);
        button_row.append(&now);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        content.append(&grid);
        content.append(&error);
        content.append(&button_row);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&now));

        let dialog = Rc::new(Self {
            id: info.id,
            window,
            name,
            size,
            dir_label,
            error,
            buttons: vec![cancel.clone(), queue.clone(), now.clone(), change_dir.clone()],
            answers: vec![queue.clone(), now.clone()],
            suggested_name: RefCell::new(info.name.clone()),
            dir: RefCell::new(info.dir.clone()),
            user_dir: Cell::new(false),
            decided: Cell::new(false),
            on_closed: RefCell::default(),
            on_answered: RefCell::default(),
        });
        dialog.show_dir();
        dialog.update(info);

        let d = Rc::downgrade(&dialog);
        now.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.answer(true);
            }
        });
        let d = Rc::downgrade(&dialog);
        queue.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                d.answer(false);
            }
        });
        let w = dialog.window.clone();
        cancel.connect_clicked(move |_| w.close());
        let d = Rc::downgrade(&dialog);
        change_dir.connect_clicked(move |_| {
            if let Some(d) = d.upgrade() {
                glib::spawn_future_local(d.choose_dir());
            }
        });

        let keys = gtk::EventControllerKey::new();
        let w = dialog.window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.window.add_controller(keys);

        let d = Rc::downgrade(&dialog);
        dialog.window.connect_close_request(move |_| {
            if let Some(d) = d.upgrade() {
                if !d.decided.replace(true) {
                    // Closing without answering = "Cancelar".
                    crate::window::send(Request::Remove { id: d.id, delete_file: false }, None);
                }
                if let Some(callback) = d.on_closed.take() {
                    callback();
                }
            }
            glib::Propagation::Proceed
        });
        dialog
    }

    pub fn connect_closed(&self, callback: impl Fn() + 'static) {
        self.on_closed.replace(Some(Box::new(callback)));
    }

    pub fn connect_answered(&self, callback: impl Fn() + 'static) {
        self.on_answered.replace(Some(Box::new(callback)));
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// New data from the daemon (size and name found by its probe).
    pub fn update(&self, info: &DownloadInfo) {
        let typed = self.name.text();
        if typed == self.suggested_name.borrow().as_str() && typed != info.name {
            self.name.set_text(&info.name);
        }
        self.suggested_name.replace(info.name.clone());
        self.size.set_label(&size_text(info));
        if !self.user_dir.get() && *self.dir.borrow() != info.dir {
            self.dir.replace(info.dir.clone());
            self.show_dir();
        }
        // The daemon refuses to confirm a download with an error (e.g. a page
        // that lists many videos); only "Cancelar" is left.
        let blocked = info.error.as_deref();
        self.error.set_label(blocked.unwrap_or_default());
        self.error.set_visible(blocked.is_some());
        for button in &self.answers {
            button.set_sensitive(blocked.is_none());
        }
    }

    /// The download stopped being pending elsewhere, or was removed.
    pub fn close_externally(&self) {
        self.decided.set(true);
        self.window.close();
    }

    fn show_dir(&self) {
        let text = match self.dir.borrow().as_ref() {
            Some(dir) => dir.display().to_string(),
            None => "pasta de Downloads".into(),
        };
        self.dir_label.set_tooltip_text(Some(&text));
        self.dir_label.set_label(&text);
    }

    async fn choose_dir(self: Rc<Self>) {
        let mut dialog = gtk::FileDialog::builder().title("Salvar em").modal(true);
        if let Some(dir) = self.dir.borrow().as_ref() {
            dialog = dialog.initial_folder(&gio::File::for_path(dir));
        }
        if let Ok(folder) = dialog.build().select_folder_future(Some(&self.window)).await {
            if let Some(path) = folder.path() {
                self.dir.replace(Some(path));
                self.user_dir.set(true);
                self.show_dir();
            }
        }
    }

    fn set_busy(&self, busy: bool) {
        for button in &self.buttons {
            button.set_sensitive(!busy);
        }
        self.name.set_sensitive(!busy);
    }

    fn answer(self: &Rc<Self>, now: bool) {
        let file_name = format::edited_name(&self.suggested_name.borrow(), &self.name.text());
        let output_dir = if self.user_dir.get() { self.dir.borrow().clone() } else { None };
        let request = Request::Confirm { id: self.id, now, file_name, output_dir };
        self.set_busy(true);
        self.error.set_visible(false);
        let this = self.clone();
        glib::spawn_future_local(async move {
            match ipc::call(request).await {
                Ok(_) => {
                    this.decided.set(true);
                    if let Some(callback) = this.on_answered.take() {
                        callback();
                    }
                    this.window.close();
                }
                Err(e) => {
                    this.error.set_label(&format!("Não foi possível confirmar: {e}"));
                    this.error.set_visible(true);
                    this.set_busy(false);
                }
            }
        });
    }
}
