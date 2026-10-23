//! Main window: views, categories and queues on the left (like IDM), the
//! download list on the right with pause, resume, remove, open file and open
//! folder (also in the right-click menu of each row, with moving between and
//! inside queues), updated live from the daemon.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use ldm_ipc::{AddRequest, Category, Direction, DownloadInfo, Queue, Request, State};

use crate::app::App;
use crate::format;
use crate::ipc;
use crate::item::DownloadItem;

/// What the list shows: a sidebar entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    All,
    Unfinished,
    Finished,
    Category(Category),
    Queue(u64),
}

impl View {
    /// The sidebar, top to bottom.
    pub fn all(queues: &[Queue]) -> Vec<View> {
        let mut views = vec![View::All, View::Unfinished, View::Finished];
        views.extend(Category::ALL.into_iter().map(View::Category));
        views.extend(queues.iter().map(|queue| View::Queue(queue.id)));
        views
    }

    pub fn label(self, queues: &[Queue]) -> String {
        match self {
            View::All => "Todos os downloads".into(),
            View::Unfinished => "Em andamento".into(),
            View::Finished => "Concluídos".into(),
            View::Category(category) => category.label().into(),
            View::Queue(id) => queues.iter().find(|q| q.id == id).map(|q| q.name.clone()).unwrap_or_default(),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            View::All => "folder-download-symbolic",
            View::Unfinished => "media-playback-start-symbolic",
            View::Finished => "object-select-symbolic",
            View::Category(category) => format::category_icon(category),
            View::Queue(_) => "view-list-symbolic",
        }
    }

    /// Heading above the first entry of a group.
    fn group(self) -> Option<&'static str> {
        match self {
            View::Category(_) => Some("Categorias"),
            View::Queue(_) => Some("Filas"),
            _ => None,
        }
    }

    pub fn matches(self, info: &DownloadInfo) -> bool {
        match self {
            View::All => true,
            View::Unfinished => info.state != State::Completed,
            View::Finished => info.state == State::Completed,
            View::Category(category) => info.category == category,
            View::Queue(id) => info.queue == id,
        }
    }
}

pub struct MainWindow {
    pub window: gtk::ApplicationWindow,
    selection: gtk::SingleSelection,
    filter: gtk::CustomFilter,
    sorter: gtk::CustomSorter,
    store: gio::ListStore,
    sidebar: gtk::ListBox,
    /// The sidebar entries, in order (queues change with the settings).
    views: Rc<RefCell<Vec<View>>>,
    current: Rc<Cell<View>>,
    counts: RefCell<Vec<(View, gtk::Label)>>,
    popover: gtk::PopoverMenu,
    status: gtk::Label,
    actions: Actions,
    /// From the settings, for the status bar.
    pub speed_limit: Cell<u64>,
    queues: RefCell<Vec<Queue>>,
}

#[derive(Clone)]
struct Actions {
    resume: gio::SimpleAction,
    pause: gio::SimpleAction,
    remove: gio::SimpleAction,
    open_file: gio::SimpleAction,
    open_folder: gio::SimpleAction,
    move_up: gio::SimpleAction,
    move_down: gio::SimpleAction,
    move_to_queue: gio::SimpleAction,
    refresh_url: gio::SimpleAction,
    /// Opens the progress window, where the download's own limit is set.
    limit_speed: gio::SimpleAction,
}

/// Opens the row menu for a list item, at a point of one of its cells.
type RowMenu = Rc<dyn Fn(&gtk::ListItem, &gtk::Widget, f64, f64)>;

/// Right click on a cell: the row menu (IDM's context menu).
fn add_row_menu(list_item: &gtk::ListItem, cell: &impl IsA<gtk::Widget>, menu: &RowMenu) {
    let gesture = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
    let (item, cell_ref, menu) = (list_item.downgrade(), cell.upcast_ref::<gtk::Widget>().downgrade(), menu.clone());
    gesture.connect_pressed(move |gesture, _, x, y| {
        if let (Some(item), Some(cell)) = (item.upgrade(), cell_ref.upgrade()) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            menu(&item, &cell, x, y);
        }
    });
    cell.add_controller(gesture);
}

fn row_menu_model(queues: &[Queue]) -> gio::Menu {
    let menu = gio::Menu::new();
    let section = gio::Menu::new();
    section.append(Some("Abrir"), Some("win.open-file"));
    section.append(Some("Abrir pasta"), Some("win.open-folder"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Retomar"), Some("win.resume"));
    section.append(Some("Pausar"), Some("win.pause"));
    section.append(Some("Atualizar endereço…"), Some("win.refresh-url"));
    section.append(Some("Limitar velocidade…"), Some("win.limit-speed"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Mover para cima"), Some("win.move-up"));
    section.append(Some("Mover para baixo"), Some("win.move-down"));
    if queues.len() > 1 {
        let targets = gio::Menu::new();
        for queue in queues {
            let item = gio::MenuItem::new(Some(&queue.name), None);
            item.set_action_and_target_value(Some("win.move-to-queue"), Some(&queue.id.to_variant()));
            targets.append_item(&item);
        }
        section.append_submenu(Some("Mover para a fila"), &targets);
    }
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Copiar endereço"), Some("win.copy-url"));
    section.append(Some("Remover…"), Some("win.remove"));
    menu.append_section(None, &section);
    menu
}

fn text_column(title: &str, property: &'static str, expand: bool, xalign: f32, menu: &RowMenu) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let menu = menu.clone();
    factory.connect_setup(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        let label = gtk::Label::builder().xalign(xalign).ellipsize(pango::EllipsizeMode::End).build();
        let expression = list_item.property_expression("item").chain_property::<DownloadItem>(property);
        expression.bind(&label, "label", gtk::Widget::NONE);
        if expand {
            expression.bind(&label, "tooltip-text", gtk::Widget::NONE);
        }
        add_row_menu(list_item, &label, &menu);
        list_item.set_child(Some(&label));
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(expand);
    column.set_resizable(true);
    column
}

fn progress_column(menu: &RowMenu) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let menu = menu.clone();
    factory.connect_setup(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        let bar = gtk::ProgressBar::builder().show_text(true).valign(gtk::Align::Center).build();
        let item = list_item.property_expression("item");
        item.chain_property::<DownloadItem>("progress").bind(&bar, "fraction", gtk::Widget::NONE);
        item.chain_property::<DownloadItem>("progress-text").bind(&bar, "text", gtk::Widget::NONE);
        add_row_menu(list_item, &bar, &menu);
        list_item.set_child(Some(&bar));
    });
    let column = gtk::ColumnViewColumn::new(Some("Progresso"), Some(factory));
    column.set_fixed_width(170);
    column.set_resizable(true);
    column
}

fn header_button(icon: &str, tooltip: &str, action: &str) -> gtk::Button {
    gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).action_name(action).build()
}

fn sidebar_row(view: View, queues: &[Queue]) -> (gtk::ListBoxRow, gtk::Label) {
    let icon = gtk::Image::from_icon_name(view.icon());
    let label = gtk::Label::builder()
        .label(view.label(queues))
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    let count = gtk::Label::new(None);
    count.add_css_class("dim-label");
    let row_box = gtk::Box::builder()
        .spacing(10)
        .margin_top(7)
        .margin_bottom(7)
        .margin_start(10)
        .margin_end(10)
        .build();
    row_box.append(&icon);
    row_box.append(&label);
    row_box.append(&count);
    let row = gtk::ListBoxRow::builder().child(&row_box).build();
    (row, count)
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let section = gio::Menu::new();
    section.append(Some("Adicionar em lote…"), Some("win.batch"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Retomar todos"), Some("win.resume-all"));
    section.append(Some("Pausar todos"), Some("win.pause-all"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Copiar endereço"), Some("win.copy-url"));
    section.append(Some("Remover concluídos da lista"), Some("win.clear-completed"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
    section.append(Some("Filas e agendador"), Some("win.queues"));
    section.append(Some("Preferências"), Some("win.preferences"));
    section.append(Some("Sobre o LDM"), Some("win.about"));
    menu.append_section(None, &section);
    menu
}

impl MainWindow {
    pub fn new(app: &Rc<App>) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(&app.app)
            .title("LDM")
            .default_width(1100)
            .default_height(600)
            .build();

        let header = gtk::HeaderBar::new();
        let add = gtk::Button::builder()
            .label("Adicionar URL")
            .tooltip_text("Adicionar um endereço (Ctrl+N)")
            .action_name("win.add")
            .build();
        add.add_css_class("suggested-action");
        header.pack_start(&add);
        header.pack_start(&header_button("media-playback-start-symbolic", "Retomar / confirmar", "win.resume"));
        header.pack_start(&header_button("media-playback-pause-symbolic", "Pausar", "win.pause"));
        header.pack_start(&header_button("user-trash-symbolic", "Remover (Delete)", "win.remove"));
        let menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu")
            .menu_model(&main_menu())
            .build();
        header.pack_end(&menu);
        header.pack_end(&header_button("folder-symbolic", "Abrir pasta", "win.open-folder"));
        header.pack_end(
            &gtk::Button::builder().label("Abrir").tooltip_text("Abrir o arquivo").action_name("win.open-file").build(),
        );
        window.set_titlebar(Some(&header));

        // Sidebar: views, categories and queues (filled by `set_queues`).
        let current = Rc::new(Cell::new(View::All));
        let views: Rc<RefCell<Vec<View>>> = Rc::default();
        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("navigation-sidebar");
        let v = views.clone();
        sidebar.set_header_func(move |row, _| {
            // "Categorias" and "Filas" above the first entry of each group.
            let views = v.borrow();
            let index = row.index() as usize;
            let group = views.get(index).and_then(|view| view.group());
            let first = group.is_some() && (index == 0 || views.get(index - 1).and_then(|view| view.group()) != group);
            match group.filter(|_| first) {
                Some(text) => {
                    let heading =
                        gtk::Label::builder().label(text).xalign(0.0).margin_start(12).margin_top(12).margin_bottom(4).build();
                    heading.add_css_class("dim-label");
                    heading.add_css_class("caption-heading");
                    row.set_header(Some(&heading));
                }
                None => row.set_header(gtk::Widget::NONE),
            }
        });
        let sidebar_scroll = gtk::ScrolledWindow::builder()
            .child(&sidebar)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(210)
            .build();

        let view_now = current.clone();
        let filter = gtk::CustomFilter::new(move |object| {
            object
                .downcast_ref::<DownloadItem>()
                .is_some_and(|item| item.with_info(|info| view_now.get().matches(info)))
        });
        let filtered = gtk::FilterListModel::new(Some(app.store.clone()), Some(filter.clone()));
        // In queue order (the order of arrival, unless moved up or down).
        let sorter = gtk::CustomSorter::new(|a, b| {
            let key = |object: &glib::Object| {
                object.downcast_ref::<DownloadItem>().map(|item| item.with_info(|info| (info.order, info.id)))
            };
            key(a).cmp(&key(b)).into()
        });
        let sorted = gtk::SortListModel::new(Some(filtered), Some(sorter.clone()));
        let selection = gtk::SingleSelection::new(Some(sorted));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        // Built with autoselect on, the first row would start selected.
        selection.set_selected(gtk::INVALID_LIST_POSITION);
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_show_column_separators(true);
        view.set_reorderable(false);
        // Right click selects the row and opens its menu; the actions follow the selection.
        let queues = app.settings().all_queues();
        let popover = gtk::PopoverMenu::builder().menu_model(&row_menu_model(&queues)).has_arrow(false).build();
        popover.set_halign(gtk::Align::Start);
        popover.set_parent(&view);
        let p = popover.clone();
        view.connect_destroy(move |_| p.unparent());
        let (s, v, popover_) = (selection.clone(), view.downgrade(), popover.clone());
        let menu: RowMenu = Rc::new(move |item, cell, x, y| {
            let popover = &popover_;
            let Some(view) = v.upgrade() else { return };
            s.set_selected(item.position());
            let (x, y) = cell.translate_coordinates(&view, x, y).unwrap_or((x, y));
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
        view.append_column(&text_column("Nome", "name", true, 0.0, &menu));
        view.append_column(&text_column("Tamanho", "size", false, 1.0, &menu));
        view.append_column(&progress_column(&menu));
        view.append_column(&text_column("Velocidade", "speed", false, 1.0, &menu));
        view.append_column(&text_column("Restante", "remaining", false, 1.0, &menu));
        view.append_column(&text_column("Estado", "status", false, 0.0, &menu));
        let list_scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();

        let paned = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&sidebar_scroll)
            .end_child(&list_scroll)
            .resize_start_child(false)
            .shrink_start_child(false)
            .position(210)
            .vexpand(true)
            .build();

        let status = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&paned);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&status);
        window.set_child(Some(&content));

        let actions = Actions {
            resume: gio::SimpleAction::new("resume", None),
            pause: gio::SimpleAction::new("pause", None),
            remove: gio::SimpleAction::new("remove", None),
            open_file: gio::SimpleAction::new("open-file", None),
            open_folder: gio::SimpleAction::new("open-folder", None),
            move_up: gio::SimpleAction::new("move-up", None),
            move_down: gio::SimpleAction::new("move-down", None),
            move_to_queue: gio::SimpleAction::new("move-to-queue", Some(glib::VariantTy::UINT64)),
            refresh_url: gio::SimpleAction::new("refresh-url", None),
            limit_speed: gio::SimpleAction::new("limit-speed", None),
        };
        for action in [
            &actions.resume,
            &actions.pause,
            &actions.remove,
            &actions.open_file,
            &actions.open_folder,
            &actions.move_up,
            &actions.move_down,
            &actions.move_to_queue,
            &actions.refresh_url,
            &actions.limit_speed,
        ] {
            window.add_action(action);
        }

        let (filter_, v, c) = (filter.clone(), views.clone(), current.clone());
        sidebar.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            let view = v.borrow().get(row.index() as usize).copied();
            if let Some(view) = view {
                c.set(view);
                filter_.changed(gtk::FilterChange::Different);
            }
        });

        let this = Self {
            window,
            selection,
            filter: filter.clone(),
            sorter,
            store: app.store.clone(),
            sidebar,
            views,
            current,
            counts: RefCell::default(),
            popover,
            status,
            actions,
            speed_limit: Cell::new(app.settings().speed_limit),
            queues: RefCell::default(),
        };
        this.connect(app, &view);
        this.set_queues(queues);
        this
    }

    /// The queues changed (settings): their sidebar entries, the row menu and
    /// the status bar follow.
    pub fn set_queues(&self, queues: Vec<Queue>) {
        let same_entries = {
            let old = self.queues.borrow();
            !self.counts.borrow().is_empty()
                && old.len() == queues.len()
                && old.iter().zip(&queues).all(|(a, b)| a.id == b.id && a.name == b.name)
        };
        self.queues.replace(queues.clone());
        if !same_entries {
            self.popover.set_menu_model(Some(&row_menu_model(&queues)));
            let views = View::all(&queues);
            self.views.replace(views.clone());
            while let Some(row) = self.sidebar.row_at_index(0) {
                self.sidebar.remove(&row);
            }
            let mut counts = Vec::new();
            for view in &views {
                let (row, count) = sidebar_row(*view, &queues);
                self.sidebar.append(&row);
                counts.push((*view, count));
            }
            self.counts.replace(counts);
            // Keep the current entry; a removed queue falls back to "Todos os downloads".
            let at = views.iter().position(|view| *view == self.current.get()).unwrap_or(0);
            self.sidebar.select_row(self.sidebar.row_at_index(at as i32).as_ref());
        }
        self.refresh(true);
    }

    fn selected(selection: &gtk::SingleSelection) -> Option<DownloadInfo> {
        selection.selected_item().and_downcast::<DownloadItem>().map(|item| item.info())
    }

    fn connect(&self, app: &Rc<App>, view: &gtk::ColumnView) {
        let app: Weak<App> = Rc::downgrade(app);
        let window = self.window.clone();
        let selection = self.selection.clone();

        let simple = |name: &str, callback: Box<dyn Fn()>| {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| callback());
            window.add_action(&action);
        };
        let parent = window.clone();
        simple("add", Box::new(move || add_url_dialog(&parent)));
        let (parent, a) = (window.clone(), app.clone());
        simple(
            "batch",
            Box::new(move || {
                if let Some(app) = a.upgrade() {
                    crate::batch::show(&parent, app.settings().all_queues());
                }
            }),
        );
        for (name, page) in [("preferences", crate::prefs::Page::General), ("queues", crate::prefs::Page::Queues)] {
            let (parent, a) = (window.clone(), app.clone());
            simple(
                name,
                Box::new(move || {
                    if let Some(app) = a.upgrade() {
                        crate::prefs::show(parent.upcast_ref(), app.settings(), page);
                    }
                }),
            );
        }
        let parent = window.clone();
        simple("about", Box::new(move || about(&parent)));
        let a = app.clone();
        simple("pause-all", Box::new(move || a.upgrade().iter().for_each(|app| app.pause_all())));
        let a = app.clone();
        simple("resume-all", Box::new(move || a.upgrade().iter().for_each(|app| app.resume_all())));
        let (parent, s) = (window.clone(), selection.clone());
        simple(
            "copy-url",
            Box::new(move || {
                if let Some(info) = Self::selected(&s) {
                    parent.clipboard().set_text(&info.url);
                }
            }),
        );
        let (parent, store) = (window.clone(), self.store.clone());
        simple(
            "clear-completed",
            Box::new(move || {
                for item in store.iter::<DownloadItem>().flatten() {
                    if item.with_info(|info| info.state == State::Completed) {
                        send(Request::Remove { id: item.info().id, delete_file: false }, Some(parent.clone().upcast()));
                    }
                }
            }),
        );

        let (s, a) = (selection.clone(), app.clone());
        self.actions.resume.connect_activate(move |_, _| {
            let Some(info) = Self::selected(&s) else { return };
            if info.state == State::Pending {
                if let Some(app) = a.upgrade() {
                    app.open_dialog(&info);
                }
            } else {
                send(Request::Resume { id: info.id }, None);
            }
        });
        let s = selection.clone();
        self.actions.pause.connect_activate(move |_, _| {
            if let Some(info) = Self::selected(&s) {
                send(Request::Pause { id: info.id }, None);
            }
        });
        for (action, direction) in [(&self.actions.move_up, Direction::Up), (&self.actions.move_down, Direction::Down)] {
            let s = selection.clone();
            action.connect_activate(move |_, _| {
                if let Some(info) = Self::selected(&s) {
                    send(Request::Reorder { id: info.id, direction }, None);
                }
            });
        }
        let (s, w) = (selection.clone(), window.clone());
        self.actions.refresh_url.connect_activate(move |_, _| {
            if let Some(info) = Self::selected(&s) {
                crate::refresh::show(w.upcast_ref(), &info);
            }
        });
        let (s, a) = (selection.clone(), app.clone());
        self.actions.limit_speed.connect_activate(move |_, _| {
            if let (Some(info), Some(app)) = (Self::selected(&s), a.upgrade()) {
                app.open_progress(info.id);
            }
        });
        let s = selection.clone();
        self.actions.move_to_queue.connect_activate(move |_, target| {
            let queue = target.and_then(|t| t.get::<u64>());
            if let (Some(info), Some(queue)) = (Self::selected(&s), queue) {
                send(Request::SetQueue { id: info.id, queue }, None);
            }
        });
        let (s, w) = (selection.clone(), window.clone());
        self.actions.remove.connect_activate(move |_, _| {
            if let Some(info) = Self::selected(&s) {
                glib::spawn_future_local(confirm_remove(w.clone(), info));
            }
        });
        let (s, w) = (selection.clone(), window.clone());
        self.actions.open_file.connect_activate(move |_, _| {
            if let Some(path) = Self::selected(&s).and_then(|info| info.path) {
                open_file(w.upcast_ref(), path);
            }
        });
        let (s, w) = (selection.clone(), window.clone());
        self.actions.open_folder.connect_activate(move |_, _| {
            if let Some(info) = Self::selected(&s) {
                open_folder(w.upcast_ref(), &info);
            }
        });

        let actions = self.actions.clone();
        self.selection.connect_selected_notify(move |selection| update_actions(&actions, Self::selected(selection)));

        // Double click / Enter: confirm a pending download, open a finished
        // one, show the progress of the others.
        view.connect_activate(move |view, position| {
            let Some(item) = view.model().and_then(|m| m.item(position)).and_downcast::<DownloadItem>() else { return };
            let info = item.info();
            let Some(app) = app.upgrade() else { return };
            match info.state {
                State::Pending => app.open_dialog(&info),
                State::Completed => {
                    if let Some(path) = info.path {
                        open_file(window.upcast_ref(), path);
                    }
                }
                _ => app.open_progress(info.id),
            }
        });
    }

    /// Re-reads the selected item (its state may have changed). `structural`:
    /// an item was added or removed, or changed state or category, so the
    /// filter and the counts must be recomputed.
    pub fn refresh(&self, structural: bool) {
        if structural {
            self.filter.changed(gtk::FilterChange::Different);
            self.sorter.changed(gtk::SorterChange::Different);
            let items: Vec<DownloadItem> = self.store.iter::<DownloadItem>().flatten().collect();
            for (view, label) in self.counts.borrow().iter() {
                let n = items.iter().filter(|item| item.with_info(|info| view.matches(info))).count();
                label.set_label(&if n > 0 { n.to_string() } else { String::new() });
            }
        }
        update_actions(&self.actions, Self::selected(&self.selection));
    }

    pub fn set_connection(&self, connection: &Result<(), String>) {
        let text = match connection {
            Err(reason) => format!("Sem conexão com o ldmd ({reason}). Tentando de novo…"),
            Ok(()) => {
                let (mut total, mut running, mut speed) = (0, 0, 0.0);
                for item in self.store.iter::<DownloadItem>().flatten() {
                    item.with_info(|info| {
                        total += 1;
                        if info.state == State::Running {
                            running += 1;
                            speed += info.speed;
                        }
                    });
                }
                let mut text = match total {
                    0 => "Nenhum download".to_string(),
                    1 => "1 download".to_string(),
                    n => format!("{n} downloads"),
                };
                if running > 0 {
                    text.push_str(&format!(" · {running} baixando · {}/s", format::bytes(speed as u64)));
                }
                if self.speed_limit.get() > 0 {
                    text.push_str(&format!(" · limite de {}/s", format::bytes(self.speed_limit.get())));
                }
                let scheduled: Vec<String> = self
                    .queues
                    .borrow()
                    .iter()
                    .filter(|queue| queue.schedule.enabled)
                    .map(|queue| format!("{}: {}", queue.name, queue.schedule.describe()))
                    .collect();
                if !scheduled.is_empty() {
                    text.push_str(&format!(" · agendado — {}", scheduled.join("; ")));
                }
                text
            }
        };
        self.status.set_label(&text);
    }
}

fn update_actions(actions: &Actions, selected: Option<DownloadInfo>) {
    let state = selected.as_ref().map(|i| i.state);
    actions.resume.set_enabled(matches!(state, Some(State::Paused | State::Failed | State::Pending)));
    actions.pause.set_enabled(matches!(state, Some(State::Running | State::Queued)));
    actions.remove.set_enabled(selected.is_some());
    actions.open_file.set_enabled(state == Some(State::Completed));
    actions.open_folder.set_enabled(selected.is_some_and(|i| i.path.is_some() || i.dir.is_some()));
    let unfinished = state.is_some_and(|s| s != State::Completed);
    actions.move_up.set_enabled(unfinished);
    actions.move_down.set_enabled(unfinished);
    actions.move_to_queue.set_enabled(state.is_some());
    actions.refresh_url.set_enabled(matches!(state, Some(State::Queued | State::Running | State::Paused | State::Failed)));
    actions.limit_speed.set_enabled(matches!(state, Some(State::Queued | State::Running | State::Paused | State::Failed)));
}

fn about(parent: &gtk::ApplicationWindow) {
    gtk::AboutDialog::builder()
        .transient_for(parent)
        .modal(true)
        .program_name("LDM")
        .comments("Gerenciador de downloads para Linux, inspirado no Internet Download Manager.")
        .version(env!("CARGO_PKG_VERSION"))
        .logo_icon_name("folder-download")
        .build()
        .present();
}

/// Esc closes the window (dialogs: Preferências, Adicionar em lote...).
pub fn close_on_escape(window: &impl IsA<gtk::Window>) {
    let keys = gtk::EventControllerKey::new();
    let w = window.as_ref().downgrade();
    keys.connect_key_pressed(move |_, key, _, _| match (key, w.upgrade()) {
        (gdk::Key::Escape, Some(window)) => {
            window.close();
            glib::Propagation::Stop
        }
        _ => glib::Propagation::Proceed,
    });
    window.as_ref().add_controller(keys);
}

/// Sends a request; failures are shown over `parent` (or logged).
pub fn send(request: Request, parent: Option<gtk::Window>) {
    glib::spawn_future_local(async move {
        if let Err(e) = ipc::call(request).await {
            show_error(parent.as_ref(), "O ldmd recusou o pedido", &e.to_string());
        }
    });
}

pub fn show_error(parent: Option<&gtk::Window>, message: &str, detail: &str) {
    eprintln!("ldm-gui: {message}: {detail}");
    gtk::AlertDialog::builder().modal(true).message(message).detail(detail).build().show(parent);
}

async fn confirm_remove(window: gtk::ApplicationWindow, info: DownloadInfo) {
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(format!("Remover \u{201c}{}\u{201d}?", info.name))
        .detail("O arquivo pode continuar na pasta ou ser apagado junto (inclusive as partes já baixadas).")
        .buttons(["Cancelar", "Remover da lista", "Remover e apagar o arquivo"])
        .cancel_button(0)
        .default_button(1)
        .build();
    let delete_file = match dialog.choose_future(Some(&window)).await {
        Ok(1) => false,
        Ok(2) => true,
        _ => return,
    };
    send(Request::Remove { id: info.id, delete_file }, Some(window.upcast()));
}

pub fn open_file(window: &gtk::Window, path: PathBuf) {
    open_file_then(window, path, || {});
}

/// Opens the file with its default app, then runs `then`. The launch is
/// asynchronous and `window` is its parent (the desktop portal asks for it):
/// closing the window before the launch finishes silently cancels it, so a
/// window that should close afterwards does it in `then`.
pub fn open_file_then(window: &gtk::Window, path: PathBuf, then: impl FnOnce() + 'static) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
    let parent = window.clone();
    launcher.launch(Some(window), gio::Cancellable::NONE, move |result| match result {
        Ok(()) => then(),
        Err(e) => show_error(Some(&parent), "Não foi possível abrir o arquivo", &e.to_string()),
    });
}

pub fn open_folder(window: &gtk::Window, info: &DownloadInfo) {
    let parent = window.clone();
    let callback = move |result: Result<(), glib::Error>| {
        if let Err(e) = result {
            show_error(Some(&parent), "Não foi possível abrir a pasta", &e.to_string());
        }
    };
    match &info.path {
        // Opens the file manager with the file selected.
        Some(path) if path.exists() => gtk::FileLauncher::new(Some(&gio::File::for_path(path)))
            .open_containing_folder(Some(window), gio::Cancellable::NONE, callback),
        path => {
            let dir = path.as_ref().and_then(|p| p.parent().map(PathBuf::from)).or_else(|| info.dir.clone());
            if let Some(dir) = dir {
                gtk::FileLauncher::new(Some(&gio::File::for_path(dir))).launch(
                    Some(window),
                    gio::Cancellable::NONE,
                    callback,
                );
            }
        }
    }
}

pub fn looks_like_url(text: &str) -> bool {
    let text = text.trim();
    ["http://", "https://", "ftp://"].iter().any(|scheme| text.starts_with(scheme)) && !text.contains(char::is_whitespace)
}

/// "Adicionar URL": the address goes to the daemon as a pending download, so the
/// usual "Novo download" window opens to confirm it.
fn add_url_dialog(parent: &gtk::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .title("Adicionar URL")
        .transient_for(parent)
        .modal(true)
        .default_width(520)
        .resizable(false)
        .build();
    let entry = gtk::Entry::builder().placeholder_text("https://…").activates_default(true).hexpand(true).build();
    let ok = gtk::Button::with_label("Adicionar");
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_label("Cancelar");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    buttons.append(&cancel);
    buttons.append(&ok);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(&gtk::Label::builder().label("Endereço do arquivo:").xalign(0.0).build());
    content.append(&entry);
    content.append(&buttons);
    dialog.set_child(Some(&content));
    dialog.set_default_widget(Some(&ok));

    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, e) = (dialog.clone(), entry.clone());
    ok.connect_clicked(move |_| {
        let url = e.text().trim().to_string();
        if url.is_empty() {
            return;
        }
        let parent = d.transient_for();
        d.close();
        send(Request::Add(AddRequest { url, confirm: true, ..Default::default() }), parent);
    });

    // Like IDM: a URL in the clipboard is suggested.
    let e = entry.clone();
    glib::spawn_future_local(async move {
        if let Ok(Some(text)) = e.clipboard().read_text_future().await {
            if e.text().is_empty() && looks_like_url(&text) {
                e.set_text(text.trim());
                e.select_region(0, -1);
            }
        }
    });
    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldm_ipc::SourceKind;

    #[test]
    fn clipboard_url_detection() {
        assert!(looks_like_url("https://example.com/a.iso"));
        assert!(looks_like_url("  ftp://example.com/a  "));
        assert!(!looks_like_url("example.com"));
        assert!(!looks_like_url("https://a b"));
    }

    #[test]
    fn sidebar_views() {
        let mut info: DownloadInfo = serde_json_like(State::Running, Category::Video);
        assert!(View::All.matches(&info));
        assert!(View::Unfinished.matches(&info));
        assert!(!View::Finished.matches(&info));
        assert!(View::Category(Category::Video).matches(&info));
        assert!(!View::Category(Category::Music).matches(&info));
        info.state = State::Completed;
        assert!(View::Finished.matches(&info) && !View::Unfinished.matches(&info));
        let queues = ldm_ipc::Settings::default().all_queues();
        let views = View::all(&queues);
        assert_eq!(views[3], View::Category(Category::Video));
        assert_eq!(views.len(), 3 + Category::ALL.len() + 1);
        assert_eq!(views.last(), Some(&View::Queue(ldm_ipc::MAIN_QUEUE)));
        assert_eq!(View::Queue(0).label(&queues), ldm_ipc::MAIN_QUEUE_NAME);
        assert_eq!(View::Queue(0).group(), Some("Filas"));
        assert!(View::Queue(0).matches(&info) && !View::Queue(1).matches(&info));
    }

    fn serde_json_like(state: State, category: Category) -> DownloadInfo {
        DownloadInfo {
            id: 1,
            url: "https://e.com/a.mp4".into(),
            kind: SourceKind::Direct,
            name: "a.mp4".into(),
            path: None,
            state,
            downloaded: 0,
            total: None,
            speed: 0.0,
            connections: 0,
            resumable: false,
            error: None,
            created_at: 0,
            dir: None,
            probing: false,
            category,
            formats: Vec::new(),
            queue: ldm_ipc::MAIN_QUEUE,
            order: 1,
            page: None,
            awaiting_url: false,
            speed_limit: 0,
        }
    }
}
