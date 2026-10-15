//! Application state: the download list mirrored from `ldmd`, the settings,
//! the main window, and the open "Novo download" and progress windows.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use ldm_ipc::{DownloadInfo, Settings, State};

use crate::confirm::ConfirmDialog;
use crate::ipc::{self, Event};
use crate::item::DownloadItem;
use crate::progress::ProgressWindow;
use crate::window::MainWindow;

/// In `--pending` mode the app quits when its last window closes; it lingers this
/// long first, in case another download arrives right after the user answered.
const LINGER: Duration = Duration::from_secs(3);

pub struct App {
    pub app: gtk::Application,
    pub store: gio::ListStore,
    items: RefCell<HashMap<u64, DownloadItem>>,
    window: RefCell<Option<MainWindow>>,
    dialogs: RefCell<HashMap<u64, Rc<ConfirmDialog>>>,
    progress: RefCell<HashMap<u64, Rc<ProgressWindow>>>,
    settings: RefCell<Settings>,
    connection: RefCell<Result<(), String>>,
    /// Keeps the app alive until the first list arrives from the daemon.
    startup_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
}

impl App {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            store: gio::ListStore::new::<DownloadItem>(),
            items: RefCell::default(),
            window: RefCell::default(),
            dialogs: RefCell::default(),
            progress: RefCell::default(),
            settings: RefCell::default(),
            connection: RefCell::new(Err("conectando ao ldmd…".into())),
            startup_hold: RefCell::default(),
        })
    }

    /// Called once, on GApplication startup.
    pub fn start(self: &Rc<Self>) {
        gtk::Window::set_default_icon_name("folder-download");
        self.app.set_accels_for_action("win.add", &["<Control>n"]);
        self.app.set_accels_for_action("win.remove", &["Delete"]);
        self.app.set_accels_for_action("win.preferences", &["<Control>comma"]);
        self.app.set_accels_for_action("win.copy-url", &["<Control>c"]);
        self.app.set_accels_for_action("window.close", &["<Control>w"]);
        self.startup_hold.replace(Some(self.app.hold()));

        let mut events = ipc::subscribe();
        let this = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(event) = events.recv().await {
                let Some(this) = this.upgrade() else { return };
                this.handle(event);
            }
        });
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    pub fn show_main_window(self: &Rc<Self>) {
        let existing = self.main_window();
        if let Some(window) = existing {
            window.present();
            return;
        }
        let window = MainWindow::new(self);
        let this = Rc::downgrade(self);
        window.window.connect_close_request(move |_| {
            if let Some(this) = this.upgrade() {
                this.window.replace(None);
            }
            glib::Propagation::Proceed
        });
        window.set_connection(&self.connection.borrow());
        window.window.present();
        self.window.replace(Some(window));
    }

    pub fn main_window(&self) -> Option<gtk::ApplicationWindow> {
        self.window.borrow().as_ref().map(|w| w.window.clone())
    }

    fn handle(self: &Rc<Self>, event: Event) {
        let mut structural = false;
        match event {
            Event::Snapshot(list) => {
                *self.connection.borrow_mut() = Ok(());
                let ids: Vec<u64> = list.iter().map(|i| i.id).collect();
                let gone: Vec<u64> = self.items.borrow().keys().copied().filter(|id| !ids.contains(id)).collect();
                for id in gone {
                    self.remove(id);
                }
                for info in list {
                    self.upsert(info);
                }
                structural = true;
                self.startup_hold.replace(None);
            }
            Event::Update(info) => structural = self.upsert(info),
            Event::Removed(id) => {
                self.remove(id);
                structural = true;
            }
            Event::Settings(settings) => {
                if let Some(window) = self.window.borrow().as_ref() {
                    window.speed_limit.set(settings.speed_limit);
                }
                *self.settings.borrow_mut() = settings;
            }
            Event::Disconnected(reason) => {
                *self.connection.borrow_mut() = Err(reason);
                self.startup_hold.replace(None);
            }
        }
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_connection(&self.connection.borrow());
            window.refresh(structural);
        }
    }

    /// Returns true when the item is new or changed state or category.
    fn upsert(self: &Rc<Self>, info: DownloadInfo) -> bool {
        self.sync_dialog(&info);
        let progress = self.progress.borrow().get(&info.id).cloned();
        if let Some(progress) = progress {
            progress.update(&info);
        }
        let existing = self.items.borrow().get(&info.id).cloned();
        match existing {
            Some(item) => {
                let changed = item.with_info(|old| old.state != info.state || old.category != info.category);
                item.update(info);
                changed
            }
            None => {
                let id = info.id;
                let item = DownloadItem::new(info);
                self.store.append(&item);
                self.items.borrow_mut().insert(id, item);
                true
            }
        }
    }

    fn remove(&self, id: u64) {
        let dialog = self.dialogs.borrow_mut().remove(&id);
        if let Some(dialog) = dialog {
            dialog.close_externally();
        }
        let progress = self.progress.borrow_mut().remove(&id);
        if let Some(progress) = progress {
            progress.close();
        }
        let Some(item) = self.items.borrow_mut().remove(&id) else { return };
        let position = (0..self.store.n_items()).find(|&i| self.store.item(i).as_ref() == Some(item.upcast_ref()));
        if let Some(position) = position {
            self.store.remove(position);
        }
    }

    /// Opens, refreshes or closes the "Novo download" window of a download.
    fn sync_dialog(self: &Rc<Self>, info: &DownloadInfo) {
        let existing = self.dialogs.borrow().get(&info.id).cloned();
        match (existing, info.state == State::Pending) {
            (Some(dialog), true) => dialog.update(info),
            (Some(dialog), false) => {
                // Answered elsewhere (CLI, another window): just close it.
                self.dialogs.borrow_mut().remove(&info.id);
                dialog.close_externally();
            }
            (None, true) => self.open_dialog(info),
            (None, false) => {}
        }
    }

    /// Keeps the app alive a moment after a window closes (see `LINGER`).
    fn linger(&self) {
        let guard = self.app.hold();
        glib::timeout_add_local_once(LINGER, move || drop(guard));
    }

    pub fn open_dialog(self: &Rc<Self>, info: &DownloadInfo) {
        if let Some(dialog) = self.dialogs.borrow().get(&info.id) {
            dialog.present();
            return;
        }
        let dialog = ConfirmDialog::new(&self.app, self.main_window().as_ref(), info);
        let this = Rc::downgrade(self);
        let id = info.id;
        dialog.connect_closed(move || {
            let Some(this) = this.upgrade() else { return };
            this.dialogs.borrow_mut().remove(&id);
            this.linger();
        });
        let this = Rc::downgrade(self);
        dialog.connect_answered(move || {
            let Some(this) = this.upgrade() else { return };
            if this.settings.borrow().show_progress {
                this.open_progress(id);
            }
        });
        self.dialogs.borrow_mut().insert(id, dialog.clone());
        dialog.present();
    }

    /// The progress window of a download (IDM's download status dialog).
    pub fn open_progress(self: &Rc<Self>, id: u64) {
        if let Some(window) = self.progress.borrow().get(&id) {
            window.present();
            return;
        }
        let Some(info) = self.items.borrow().get(&id).map(|item| item.info()) else { return };
        let window = ProgressWindow::new(&self.app, self.main_window().as_ref(), &info);
        let this = Rc::downgrade(self);
        window.connect_closed(move || {
            let Some(this) = this.upgrade() else { return };
            this.progress.borrow_mut().remove(&id);
            this.linger();
        });
        self.progress.borrow_mut().insert(id, window.clone());
        window.present();
    }
}
