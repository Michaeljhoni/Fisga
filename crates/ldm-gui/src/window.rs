//! Main window: categories on the left (like IDM), the download list on the
//! right with pause, resume, remove, open file and open folder, updated live
//! from the daemon.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use ldm_ipc::{AddRequest, Category, DownloadInfo, Request, State};

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
}

impl View {
    pub fn all() -> Vec<View> {
        let mut views = vec![View::All, View::Unfinished, View::Finished];
        views.extend(Category::ALL.into_iter().map(View::Category));
        views
    }

    pub fn label(self) -> &'static str {
        match self {
            View::All => "Todos os downloads",
            View::Unfinished => "Em andamento",
            View::Finished => "Concluídos",
            View::Category(category) => category.label(),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            View::All => "folder-download-symbolic",
            View::Unfinished => "media-playback-start-symbolic",
            View::Finished => "object-select-symbolic",
            View::Category(category) => format::category_icon(category),
        }
    }

    pub fn matches(self, info: &DownloadInfo) -> bool {
        match self {
            View::All => true,
            View::Unfinished => info.state != State::Completed,
            View::Finished => info.state == State::Completed,
            View::Category(category) => info.category == category,
        }
    }
}

pub struct MainWindow {
    pub window: gtk::ApplicationWindow,
    selection: gtk::SingleSelection,
    filter: gtk::CustomFilter,
    store: gio::ListStore,
    counts: Vec<(View, gtk::Label)>,
    status: gtk::Label,
    actions: Actions,
}

#[derive(Clone)]
struct Actions {
    resume: gio::SimpleAction,
    pause: gio::SimpleAction,
    remove: gio::SimpleAction,
    open_file: gio::SimpleAction,
    open_folder: gio::SimpleAction,
}

fn text_column(title: &str, property: &'static str, expand: bool, xalign: f32) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        let label = gtk::Label::builder().xalign(xalign).ellipsize(pango::EllipsizeMode::End).build();
        let expression = list_item.property_expression("item").chain_property::<DownloadItem>(property);
        expression.bind(&label, "label", gtk::Widget::NONE);
        if expand {
            expression.bind(&label, "tooltip-text", gtk::Widget::NONE);
        }
        list_item.set_child(Some(&label));
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_expand(expand);
    column.set_resizable(true);
    column
}

fn progress_column() -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, object| {
        let list_item = object.downcast_ref::<gtk::ListItem>().expect("a ListItem");
        let bar = gtk::ProgressBar::builder().show_text(true).valign(gtk::Align::Center).build();
        let item = list_item.property_expression("item");
        item.chain_property::<DownloadItem>("progress").bind(&bar, "fraction", gtk::Widget::NONE);
        item.chain_property::<DownloadItem>("progress-text").bind(&bar, "text", gtk::Widget::NONE);
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

fn sidebar_row(view: View) -> (gtk::ListBoxRow, gtk::Label) {
    let icon = gtk::Image::from_icon_name(view.icon());
    let label = gtk::Label::builder().label(view.label()).xalign(0.0).hexpand(true).build();
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
    section.append(Some("Remover concluídos da lista"), Some("win.clear-completed"));
    menu.append_section(None, &section);
    let section = gio::Menu::new();
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
        header.pack_end(&header_button("folder-open-symbolic", "Abrir pasta", "win.open-folder"));
        header.pack_end(&header_button("document-open-symbolic", "Abrir arquivo", "win.open-file"));
        window.set_titlebar(Some(&header));

        // Sidebar: views and categories.
        let current = Rc::new(Cell::new(View::All));
        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("navigation-sidebar");
        let mut counts = Vec::new();
        for view in View::all() {
            let (row, count) = sidebar_row(view);
            sidebar.append(&row);
            counts.push((view, count));
        }
        sidebar.set_header_func(|row, _| {
            // A "Categorias" heading above the first category.
            if row.index() == 3 {
                let heading = gtk::Label::builder()
                    .label("Categorias")
                    .xalign(0.0)
                    .margin_start(12)
                    .margin_top(12)
                    .margin_bottom(4)
                    .build();
                heading.add_css_class("dim-label");
                heading.add_css_class("caption-heading");
                row.set_header(Some(&heading));
            } else {
                row.set_header(gtk::Widget::NONE);
            }
        });
        sidebar.select_row(sidebar.row_at_index(0).as_ref());
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
        let selection = gtk::SingleSelection::new(Some(filtered));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        // Built with autoselect on, the first row would start selected.
        selection.set_selected(gtk::INVALID_LIST_POSITION);
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_show_column_separators(true);
        view.set_reorderable(false);
        view.append_column(&text_column("Nome", "name", true, 0.0));
        view.append_column(&text_column("Tamanho", "size", false, 1.0));
        view.append_column(&progress_column());
        view.append_column(&text_column("Velocidade", "speed", false, 1.0));
        view.append_column(&text_column("Restante", "remaining", false, 1.0));
        view.append_column(&text_column("Estado", "status", false, 0.0));
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
        };
        for action in [&actions.resume, &actions.pause, &actions.remove, &actions.open_file, &actions.open_folder] {
            window.add_action(action);
        }

        let this = Self {
            window,
            selection,
            filter: filter.clone(),
            store: app.store.clone(),
            counts,
            status,
            actions,
        };
        this.connect(app, &view);

        let filter_ = filter.clone();
        sidebar.connect_row_selected(move |_, row| {
            let Some(row) = row else { return };
            if let Some(view) = View::all().get(row.index() as usize) {
                current.set(*view);
                filter_.changed(gtk::FilterChange::Different);
            }
        });
        this.refresh(true);
        this
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
            "preferences",
            Box::new(move || {
                if let Some(app) = a.upgrade() {
                    crate::prefs::show(parent.upcast_ref(), app.settings());
                }
            }),
        );
        let parent = window.clone();
        simple("about", Box::new(move || about(&parent)));
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
            let items: Vec<DownloadItem> = self.store.iter::<DownloadItem>().flatten().collect();
            for (view, label) in &self.counts {
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
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
    let parent = window.clone();
    launcher.launch(Some(window), gio::Cancellable::NONE, move |result| {
        if let Err(e) = result {
            show_error(Some(&parent), "Não foi possível abrir o arquivo", &e.to_string());
        }
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

fn looks_like_url(text: &str) -> bool {
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
        // "Categorias" heading sits above index 3: the first category.
        assert_eq!(View::all()[3], View::Category(Category::Video));
        assert_eq!(View::all().len(), 3 + Category::ALL.len());
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
        }
    }
}
