//! "Novo download" window, shown for every pending download (IDM's "Download
//! File Info"): name, size and destination folder, the queue (when there is
//! more than one), the quality of a site video, what to do when the file
//! already exists, with "Baixar agora", "Adicionar à fila" and "Cancelar".
//! Closing the window cancels.
//!
//! The title is always exactly `Novo download`, so compositors can match it
//! (niri: `window-rule { match app-id="io.ldm.Ldm" title="^Novo download$" ... }`).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use ldm_ipc::{Category, Conflict, DownloadInfo, Existing, MediaFormat, Queue, Request, SourceKind};

use crate::format;
use crate::ipc;

pub const TITLE: &str = "Novo download";

pub struct ConfirmDialog {
    id: u64,
    kind: SourceKind,
    window: gtk::Window,
    name: gtk::Entry,
    size: gtk::Label,
    dir_label: gtk::Label,
    error: gtk::Label,
    /// Qualities of a site video; the row shows when there is a choice.
    formats: RefCell<Vec<MediaFormat>>,
    quality: gtk::DropDown,
    quality_caption: gtk::Label,
    /// The queue, with the ids of its entries; hidden with only the main queue.
    queue: gtk::DropDown,
    queue_ids: Vec<u64>,
    /// "Já existe": shown when the destination file is already there.
    exists_caption: gtk::Label,
    exists_box: gtk::Box,
    exists_label: gtk::Label,
    exists_choice: gtk::DropDown,
    /// What each entry of `exists_choice` means.
    choices: RefCell<Vec<Conflict>>,
    existing: Cell<Existing>,
    buttons: Vec<gtk::Button>,
    /// "Adicionar à fila" and "Baixar agora": disabled while the daemon reports an error.
    answers: Vec<gtk::Button>,
    /// Name last suggested by the daemon; the entry follows it until the user edits it.
    suggested_name: RefCell<String>,
    /// Folder shown; follows the daemon (category subfolder) until the user picks one.
    dir: RefCell<Option<PathBuf>>,
    user_dir: Cell<bool>,
    /// The daemon's folder and the category it is for: a name (or quality)
    /// of another type moves the shown folder, as the daemon will on confirming.
    daemon_dir: RefCell<Option<PathBuf>>,
    daemon_category: Cell<Category>,
    category_folders: bool,
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
    pub fn new(
        app: &gtk::Application,
        parent: Option<&gtk::ApplicationWindow>,
        info: &DownloadInfo,
        category_folders: bool,
        queues: Vec<Queue>,
    ) -> Rc<Self> {
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
        let quality = gtk::DropDown::builder().halign(gtk::Align::Start).visible(false).build();
        let quality_caption = caption("Qualidade");
        quality_caption.set_visible(false);
        let exists_label = gtk::Label::builder().xalign(0.0).wrap(true).build();
        exists_label.add_css_class("warning");
        let exists_choice = gtk::DropDown::builder().halign(gtk::Align::Start).build();
        let exists_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).visible(false).build();
        exists_box.append(&exists_label);
        exists_box.append(&exists_choice);
        let exists_caption = caption("Já existe");
        exists_caption.set_visible(false);
        exists_caption.set_valign(gtk::Align::Start);
        let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
        let queue_choice = gtk::DropDown::from_strings(&names);
        queue_choice.set_halign(gtk::Align::Start);
        queue_choice.set_selected(queues.iter().position(|q| q.id == info.queue).unwrap_or(0) as u32);
        let queue_caption = caption("Fila");
        queue_choice.set_visible(queues.len() > 1);
        queue_caption.set_visible(queues.len() > 1);

        let grid = gtk::Grid::builder().column_spacing(12).row_spacing(10).build();
        for (row, (label, widget)) in [
            (caption("Endereço"), url.upcast_ref::<gtk::Widget>()),
            (caption("Salvar como"), name.upcast_ref()),
            (caption("Pasta"), dir_row.upcast_ref()),
            (queue_caption, queue_choice.upcast_ref()),
            (quality_caption.clone(), quality.upcast_ref()),
            (caption("Tamanho"), size.upcast_ref()),
            (exists_caption.clone(), exists_box.upcast_ref()),
        ]
        .into_iter()
        .enumerate()
        {
            grid.attach(&label, 0, row as i32, 1, 1);
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
            kind: info.kind,
            window,
            name,
            size,
            dir_label,
            error,
            formats: RefCell::default(),
            quality,
            quality_caption,
            queue: queue_choice,
            queue_ids: queues.iter().map(|q| q.id).collect(),
            exists_caption,
            exists_box,
            exists_label,
            exists_choice,
            choices: RefCell::default(),
            existing: Cell::new(Existing::Nothing),
            buttons: vec![cancel.clone(), queue.clone(), now.clone(), change_dir.clone()],
            answers: vec![queue.clone(), now.clone()],
            suggested_name: RefCell::new(info.name.clone()),
            dir: RefCell::new(info.dir.clone()),
            user_dir: Cell::new(false),
            daemon_dir: RefCell::new(info.dir.clone()),
            daemon_category: Cell::new(info.category),
            category_folders,
            decided: Cell::new(false),
            on_closed: RefCell::default(),
            on_answered: RefCell::default(),
        });
        dialog.show_dir();
        dialog.update(info);
        // Start in the name field (the selectable URL label would grab focus and look selected).
        GtkWindowExt::set_focus(&dialog.window, Some(&dialog.name));

        let d = Rc::downgrade(&dialog);
        dialog.quality.connect_selected_notify(move |_| {
            if let Some(d) = d.upgrade() {
                d.quality_changed();
            }
        });
        let d = Rc::downgrade(&dialog);
        dialog.name.connect_changed(move |_| {
            if let Some(d) = d.upgrade() {
                d.refresh_dir();
            }
        });

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
        self.set_formats(&info.formats);
        self.show_format_size();
        self.daemon_dir.replace(info.dir.clone());
        self.daemon_category.set(info.category);
        self.refresh_dir();
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
        self.check_existing();
    }

    /// The folder follows the daemon and the type of the name, until the user picks one.
    fn refresh_dir(&self) {
        if !self.user_dir.get() {
            let category = Category::detect(self.name.text().trim(), self.kind);
            let dir = self.daemon_dir.borrow().as_deref().map(|dir| {
                format::category_dir(dir, self.daemon_category.get(), category, self.category_folders)
            });
            if *self.dir.borrow() != dir {
                self.dir.replace(dir);
                self.show_dir();
                return;
            }
        }
        self.check_existing();
    }

    fn set_formats(&self, formats: &[MediaFormat]) {
        if *self.formats.borrow() == formats {
            return;
        }
        self.formats.replace(formats.to_vec());
        let labels: Vec<String> = formats
            .iter()
            .map(|f| match f.size {
                Some(size) => format!("{} \u{2014} \u{2248} {}", f.label, format::bytes(size)),
                None => f.label.clone(),
            })
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.quality.set_model(Some(&gtk::StringList::new(&labels)));
        self.quality.set_selected(0);
        // One quality is no choice.
        let choice = formats.len() > 1;
        self.quality.set_visible(choice);
        self.quality_caption.set_visible(choice);
        self.show_format_size();
    }

    fn selected_format(&self) -> Option<MediaFormat> {
        let formats = self.formats.borrow();
        (formats.len() > 1).then(|| formats.get(self.quality.selected() as usize).cloned()).flatten()
    }

    fn show_format_size(&self) {
        if let Some(format) = self.selected_format() {
            self.size.set_label(&match format.size {
                Some(size) => format!("\u{2248} {}", format::bytes(size)),
                None => "definido ao baixar".into(),
            });
        }
    }

    /// Another quality: its size, and its extension in the name (audio is .m4a).
    fn quality_changed(&self) {
        let Some(format) = self.selected_format() else { return };
        self.show_format_size();
        if let Some(ext) = &format.ext {
            let name = format::with_extension(self.name.text().trim(), ext);
            if name != self.name.text() {
                self.name.set_text(&name);
            }
        }
    }

    /// Asks what to do when the file is already in the folder, as IDM does.
    fn check_existing(&self) {
        let name = self.name.text().trim().replace('/', "_");
        let existing = match self.dir.borrow().as_ref() {
            Some(dir) if !name.is_empty() => ldm_ipc::existing(&dir.join(&name)),
            _ => Existing::Nothing,
        };
        let visible = existing != Existing::Nothing;
        self.exists_box.set_visible(visible);
        self.exists_caption.set_visible(visible);
        if !visible {
            self.existing.set(existing);
            return;
        }
        let dir = self.dir.borrow().clone().unwrap_or_default();
        let other_name = format!("Salvar como \u{201c}{}\u{201d}", ldm_ipc::unique_name(&dir, &name));
        let (message, entries): (&str, Vec<(Conflict, &str)>) = match existing {
            Existing::File => (
                "Já existe um arquivo com este nome nesta pasta.",
                vec![(Conflict::Rename, other_name.as_str()), (Conflict::Overwrite, "Substituir o arquivo existente")],
            ),
            _ => (
                "Há um download incompleto deste arquivo nesta pasta.",
                vec![
                    (Conflict::Resume, "Continuar de onde parou"),
                    (Conflict::Rename, other_name.as_str()),
                    (Conflict::Overwrite, "Recomeçar do zero"),
                ],
            ),
        };
        let keep = if self.existing.replace(existing) == existing { self.exists_choice.selected() } else { 0 };
        self.exists_label.set_label(message);
        let labels: Vec<&str> = entries.iter().map(|(_, label)| *label).collect();
        self.choices.replace(entries.iter().map(|(choice, _)| *choice).collect());
        self.exists_choice.set_model(Some(&gtk::StringList::new(&labels)));
        self.exists_choice.set_selected(keep.min(labels.len() as u32 - 1));
    }

    fn conflict(&self) -> Option<Conflict> {
        if !self.exists_box.is_visible() {
            return None;
        }
        self.choices.borrow().get(self.exists_choice.selected() as usize).copied()
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
        self.quality.set_sensitive(!busy);
        self.queue.set_sensitive(!busy);
        self.exists_choice.set_sensitive(!busy);
    }

    fn answer(self: &Rc<Self>, now: bool) {
        let file_name = format::edited_name(&self.suggested_name.borrow(), &self.name.text());
        let output_dir = if self.user_dir.get() { self.dir.borrow().clone() } else { None };
        // The first quality is the best one, which yt-dlp picks by itself.
        let format = self.selected_format().filter(|_| self.quality.selected() > 0).map(|f| f.key);
        let conflict = self.conflict();
        let queue = self.queue_ids.get(self.queue.selected() as usize).copied().filter(|_| self.queue.is_visible());
        let request = Request::Confirm { id: self.id, now, file_name, output_dir, format, conflict, queue };
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
