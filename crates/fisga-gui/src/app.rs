//! Application state: the download list mirrored from `fisgad`, the settings,
//! the main window, the open "Novo download" and progress windows, and the
//! tray icon.
//!
//! Lifetime: `fisga-gui` quits when its last window closes, unless it lives in
//! the tray (or was started with `--background`), where it keeps answering
//! the browser's downloads until "Sair".

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gio, glib};
use fisga_ipc::{DownloadInfo, Request, Settings, State, Theme};
use tokio::sync::mpsc;

use crate::confirm::ConfirmDialog;
use crate::festive::Season;
use crate::ipc::{self, Event};
use crate::item::DownloadItem;
use crate::power::PowerDialog;
use crate::progress::ProgressWindow;
use crate::tray;
use crate::window::{send, MainWindow};

/// In `--pending` mode the app quits when its last window closes; it lingers this
/// long first, in case another download arrives right after the user answered.
const LINGER: Duration = Duration::from_secs(3);

enum TrayState {
    Off,
    Starting,
    On(tray::Handle),
    /// No tray host when the main window opened: do not retry until the
    /// setting changes.
    Failed,
}

pub struct App {
    pub app: gtk::Application,
    pub store: gio::ListStore,
    items: RefCell<HashMap<u64, DownloadItem>>,
    window: RefCell<Option<MainWindow>>,
    dialogs: RefCell<HashMap<u64, Rc<ConfirmDialog>>>,
    progress: RefCell<HashMap<u64, Rc<ProgressWindow>>>,
    power: RefCell<Option<PowerDialog>>,
    settings: RefCell<Settings>,
    settings_loaded: Cell<bool>,
    connection: RefCell<Result<(), String>>,
    /// Keeps the app alive until the first list arrives from the daemon.
    startup_hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    /// `--background`: stays running without windows.
    background: Cell<bool>,
    /// The main window was opened or `--background` was given: the tray icon
    /// belongs here (not in a short-lived `--pending` instance).
    persistent: Cell<bool>,
    tray: RefCell<TrayState>,
    tray_actions: mpsc::UnboundedSender<tray::Action>,
    tray_receiver: RefCell<Option<mpsc::UnboundedReceiver<tray::Action>>>,
    tray_summary: RefCell<String>,
    /// Held while the app lives in the tray or in the background.
    keep_alive: RefCell<Option<gio::ApplicationHoldGuard>>,
}

impl App {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let (tray_actions, tray_receiver) = mpsc::unbounded_channel();
        Rc::new(Self {
            app: app.clone(),
            store: gio::ListStore::new::<DownloadItem>(),
            items: RefCell::default(),
            window: RefCell::default(),
            dialogs: RefCell::default(),
            progress: RefCell::default(),
            power: RefCell::default(),
            settings: RefCell::default(),
            settings_loaded: Cell::new(false),
            connection: RefCell::new(Err("conectando ao fisgad…".into())),
            startup_hold: RefCell::default(),
            background: Cell::new(false),
            persistent: Cell::new(false),
            tray: RefCell::new(TrayState::Off),
            tray_actions,
            tray_receiver: RefCell::new(Some(tray_receiver)),
            tray_summary: RefCell::default(),
            keep_alive: RefCell::default(),
        })
    }

    /// Called once, on GApplication startup.
    pub fn start(self: &Rc<Self>) {
        gtk::Window::set_default_icon_name("folder-download");
        crate::theme::load();
        self.app.set_accels_for_action("win.add", &["<Control>n"]);
        self.app.set_accels_for_action("win.batch", &["<Control><Shift>n"]);
        self.app.set_accels_for_action("win.find", &["<Control>f"]);
        self.app.set_accels_for_action("win.grab", &["<Control><Shift>g"]);
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

        let mut actions = self.tray_receiver.take().expect("start runs once");
        let this = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(action) = actions.recv().await {
                let Some(this) = this.upgrade() else { return };
                this.tray_action(action);
            }
        });

        // The tray tooltip follows the downloads, once a second.
        let this = Rc::downgrade(self);
        glib::timeout_add_seconds_local(1, move || {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            this.refresh_tray();
            glib::ControlFlow::Continue
        });

        // A holiday's week begins or ends while the app runs (in the tray, for days).
        let this = Rc::downgrade(self);
        glib::timeout_add_seconds_local(300, move || {
            let Some(this) = this.upgrade() else { return glib::ControlFlow::Break };
            this.sync_look();
            glib::ControlFlow::Continue
        });
    }

    /// The theme of the settings, or the holiday's in its week.
    fn sync_look(&self) {
        if !self.settings_loaded.get() {
            return;
        }
        let settings = self.settings.borrow();
        let season = if settings.festive { Season::today() } else { None };
        crate::theme::set_style(settings.theme == Theme::Light, season);
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    /// The daemon sent its settings: a change made to them can be sent back
    /// (before, it would overwrite the user's with the defaults).
    pub fn has_settings(&self) -> bool {
        self.settings_loaded.get()
    }

    /// `fisga-gui --background` (session autostart): no window, only the tray.
    pub fn run_in_background(self: &Rc<Self>) {
        self.background.set(true);
        self.persistent.set(true);
        self.update_hold();
        self.sync_tray();
    }

    pub fn show_main_window(self: &Rc<Self>) {
        self.persistent.set(true);
        self.sync_tray();
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
        self.sync_look();
    }

    pub fn main_window(&self) -> Option<gtk::ApplicationWindow> {
        self.window.borrow().as_ref().map(|w| w.window.clone())
    }

    /// "Pausar todos" / "Retomar todos" (IDM's "Parar/Iniciar todos").
    pub fn pause_all(&self) {
        self.for_each_in(&[State::Running, State::Queued], |id| Request::Pause { id });
    }

    pub fn resume_all(&self) {
        self.for_each_in(&[State::Paused, State::Failed], |id| Request::Resume { id });
    }

    fn for_each_in(&self, states: &[State], make: fn(u64) -> Request) {
        for item in self.store.iter::<DownloadItem>().flatten() {
            let (id, state) = item.with_info(|info| (info.id, info.state));
            if states.contains(&state) {
                send(make(id), None);
            }
        }
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
                    window.set_queues(settings.all_queues());
                    window.set_hidden_columns(&settings.hidden_columns);
                }
                let queues = settings.all_queues();
                let dialogs: Vec<Rc<ConfirmDialog>> = self.dialogs.borrow().values().cloned().collect();
                for dialog in dialogs {
                    dialog.set_queues(&queues);
                }
                if !settings.tray && matches!(*self.tray.borrow(), TrayState::Failed) {
                    self.tray.replace(TrayState::Off);
                }
                let (list_view, newest_first) = (settings.list_view, settings.newest_first);
                *self.settings.borrow_mut() = settings;
                self.settings_loaded.set(true);
                self.sync_look();
                // After storing them: the cards/list toggle compares with the stored settings.
                if let Some(window) = self.window.borrow().as_ref() {
                    window.set_view(list_view, newest_first);
                }
                self.sync_tray();
            }
            Event::Power(plan) => self.sync_power(plan),
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

    /// Returns true when the item is new or changed state, category, queue or position.
    fn upsert(self: &Rc<Self>, info: DownloadInfo) -> bool {
        self.sync_dialog(&info);
        let progress = self.progress.borrow().get(&info.id).cloned();
        if let Some(progress) = progress {
            progress.update(&info);
        }
        let existing = self.items.borrow().get(&info.id).cloned();
        match existing {
            Some(item) => {
                let changed = item.with_info(|old| {
                    (old.state, old.category, old.queue, old.order) != (info.state, info.category, info.queue, info.order)
                });
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

    /// The countdown window of a planned suspend/power off.
    fn sync_power(self: &Rc<Self>, plan: Option<fisga_ipc::PowerPlan>) {
        let open = self.power.borrow_mut().take();
        match (open, plan) {
            (Some(dialog), Some(plan)) if dialog.plan == plan && dialog.is_open() => {
                dialog.present();
                self.power.replace(Some(dialog));
            }
            (open, plan) => {
                if let Some(dialog) = open {
                    dialog.close_externally();
                    self.linger();
                }
                if let Some(plan) = plan {
                    let dialog = PowerDialog::new(&self.app, self.main_window().as_ref(), plan);
                    dialog.present();
                    self.power.replace(Some(dialog));
                }
            }
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
        let settings = self.settings();
        let dialog =
            ConfirmDialog::new(&self.app, self.main_window().as_ref(), info, settings.category_folders, settings.all_queues());
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

    /// Starts or stops the tray icon to match the settings.
    fn sync_tray(self: &Rc<Self>) {
        if !self.settings_loaded.get() {
            return;
        }
        let wanted = self.persistent.get() && self.settings.borrow().tray;
        let current = self.tray.replace(TrayState::Off);
        match (wanted, current) {
            (true, TrayState::Off) => {
                self.tray.replace(TrayState::Starting);
                let this = Rc::downgrade(self);
                let (actions, summary) = (self.tray_actions.clone(), self.summary());
                // At login the tray host may start after us: wait for it in the background.
                let wait_for_host = self.background.get();
                glib::spawn_future_local(async move {
                    let result = tray::spawn(actions, summary, wait_for_host).await;
                    let Some(this) = this.upgrade() else { return };
                    let still_wanted = this.persistent.get() && this.settings.borrow().tray;
                    match result {
                        Ok(handle) if still_wanted => {
                            this.tray.replace(TrayState::On(handle));
                        }
                        Ok(handle) => {
                            this.tray.replace(TrayState::Off);
                            tray::shutdown(handle).await;
                        }
                        Err(e) => {
                            eprintln!("fisga-gui: sem ícone na bandeja: {e}");
                            this.tray.replace(TrayState::Failed);
                        }
                    }
                    this.update_hold();
                });
            }
            (false, TrayState::On(handle)) => {
                glib::spawn_future_local(tray::shutdown(handle));
            }
            // Starting: the result is checked against the settings when it arrives.
            (_, state) => {
                self.tray.replace(state);
            }
        }
        self.update_hold();
    }

    fn update_hold(&self) {
        let keep = self.background.get() || matches!(*self.tray.borrow(), TrayState::On(_));
        let held = self.keep_alive.borrow().is_some();
        if keep && !held {
            self.keep_alive.replace(Some(self.app.hold()));
        } else if !keep && held {
            self.keep_alive.replace(None);
        }
    }

    fn summary(&self) -> String {
        let infos: Vec<DownloadInfo> = self.store.iter::<DownloadItem>().flatten().map(|item| item.info()).collect();
        crate::format::activity(&infos)
    }

    fn refresh_tray(&self) {
        let TrayState::On(handle) = &*self.tray.borrow() else { return };
        let summary = self.summary();
        if *self.tray_summary.borrow() == summary {
            return;
        }
        self.tray_summary.replace(summary.clone());
        let handle = handle.clone();
        glib::spawn_future_local(async move { tray::set_summary(&handle, summary).await });
    }

    fn tray_action(self: &Rc<Self>, action: tray::Action) {
        match action {
            tray::Action::Open => self.show_main_window(),
            tray::Action::Add => {
                self.show_main_window();
                if let Some(window) = self.main_window() {
                    let _ = WidgetExt::activate_action(&window, "win.add", None);
                }
            }
            tray::Action::ResumeAll => self.resume_all(),
            tray::Action::PauseAll => self.pause_all(),
            // Only the interface quits: the downloads go on in fisgad.
            tray::Action::Quit => {
                if let TrayState::On(handle) = self.tray.replace(TrayState::Off) {
                    glib::spawn_future_local(tray::shutdown(handle));
                }
                self.app.quit();
            }
        }
    }
}
