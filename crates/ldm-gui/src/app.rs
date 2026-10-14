//! Application state: the download list mirrored from `ldmd`, the main window
//! and the open "Novo download" windows.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use ldm_ipc::{DownloadInfo, State};

use crate::confirm::ConfirmDialog;
use crate::ipc::{self, Event};
use crate::item::DownloadItem;
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
            connection: RefCell::new(Err("conectando ao ldmd…".into())),
            startup_hold: RefCell::default(),
        })
    }

    /// Called once, on GApplication startup.
    pub fn start(self: &Rc<Self>) {
        gtk::Window::set_default_icon_name("folder-download");
        self.app.set_accels_for_action("win.add", &["<Control>n"]);
        self.app.set_accels_for_action("win.remove", &["Delete"]);
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

    pub fn show_main_window(self: &Rc<Self>) {
        let existing = self.window.borrow().as_ref().map(|w| w.window.clone());
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
                self.startup_hold.replace(None);
            }
            Event::Update(info) => self.upsert(info),
            Event::Removed(id) => self.remove(id),
            Event::Disconnected(reason) => {
                *self.connection.borrow_mut() = Err(reason);
                self.startup_hold.replace(None);
            }
        }
        if let Some(window) = self.window.borrow().as_ref() {
            window.set_connection(&self.connection.borrow());
            window.refresh();
        }
    }

    fn upsert(self: &Rc<Self>, info: DownloadInfo) {
        self.sync_dialog(&info);
        let existing = self.items.borrow().get(&info.id).cloned();
        match existing {
            Some(item) => item.update(info),
            None => {
                let id = info.id;
                let item = DownloadItem::new(info);
                self.store.append(&item);
                self.items.borrow_mut().insert(id, item);
            }
        }
    }

    fn remove(&self, id: u64) {
        let dialog = self.dialogs.borrow_mut().remove(&id);
        if let Some(dialog) = dialog {
            dialog.close_externally();
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
            let guard = this.app.hold();
            glib::timeout_add_local_once(LINGER, move || drop(guard));
        });
        self.dialogs.borrow_mut().insert(id, dialog.clone());
        dialog.present();
    }
}
