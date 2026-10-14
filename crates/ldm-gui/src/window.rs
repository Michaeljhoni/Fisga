//! Main window: the download list with pause, resume, remove, open file and
//! open folder, updated live from the daemon.

use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use ldm_ipc::{AddRequest, DownloadInfo, Request, State};

use crate::app::App;
use crate::format;
use crate::ipc;
use crate::item::DownloadItem;

pub struct MainWindow {
    pub window: gtk::ApplicationWindow,
    selection: gtk::SingleSelection,
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

impl MainWindow {
    pub fn new(app: &Rc<App>) -> Self {
        let window = gtk::ApplicationWindow::builder()
            .application(&app.app)
            .title("LDM")
            .default_width(960)
            .default_height(520)
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
        header.pack_end(&header_button("folder-open-symbolic", "Abrir pasta", "win.open-folder"));
        header.pack_end(&header_button("document-open-symbolic", "Abrir arquivo", "win.open-file"));
        window.set_titlebar(Some(&header));

        let selection = gtk::SingleSelection::new(Some(app.store.clone()));
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
        let scrolled = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();

        let status = gtk::Label::builder()
            .xalign(0.0)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&scrolled);
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
        let add_action = gio::SimpleAction::new("add", None);
        for action in [
            &add_action,
            &actions.resume,
            &actions.pause,
            &actions.remove,
            &actions.open_file,
            &actions.open_folder,
        ] {
            window.add_action(action);
        }

        let this = Self { window, selection, status, actions };
        this.connect(app, &add_action, &view);
        this.refresh();
        this
    }

    fn selected(selection: &gtk::SingleSelection) -> Option<DownloadInfo> {
        selection.selected_item().and_downcast::<DownloadItem>().map(|item| item.info())
    }

    fn connect(&self, app: &Rc<App>, add: &gio::SimpleAction, view: &gtk::ColumnView) {
        let app: Weak<App> = Rc::downgrade(app);
        let window = self.window.clone();
        let selection = self.selection.clone();

        let parent = window.clone();
        add.connect_activate(move |_, _| add_url_dialog(&parent));

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
                open_file(&w, path);
            }
        });
        let (s, w) = (selection.clone(), window.clone());
        self.actions.open_folder.connect_activate(move |_, _| {
            if let Some(info) = Self::selected(&s) {
                open_folder(&w, &info);
            }
        });

        let actions = self.actions.clone();
        self.selection.connect_selected_notify(move |selection| update_actions(&actions, Self::selected(selection)));

        view.connect_activate(move |view, position| {
            let Some(item) = view.model().and_then(|m| m.item(position)).and_downcast::<DownloadItem>() else { return };
            let info = item.info();
            match info.state {
                State::Pending => {
                    if let Some(app) = app.upgrade() {
                        app.open_dialog(&info);
                    }
                }
                State::Completed => {
                    if let Some(path) = info.path {
                        open_file(&window, path);
                    }
                }
                _ => {}
            }
        });
    }

    /// Re-reads the selected item (its state may have changed) and the totals.
    pub fn refresh(&self) {
        update_actions(&self.actions, Self::selected(&self.selection));
    }

    pub fn set_connection(&self, connection: &Result<(), String>) {
        let text = match connection {
            Err(reason) => format!("Sem conexão com o ldmd ({reason}). Tentando de novo…"),
            Ok(()) => {
                let model = self.selection.model().expect("model set in new()");
                let infos: Vec<DownloadInfo> = (0..model.n_items())
                    .filter_map(|i| model.item(i).and_downcast::<DownloadItem>())
                    .map(|item| item.info())
                    .collect();
                let running = infos.iter().filter(|i| i.state == State::Running).count();
                let speed: f64 = infos.iter().filter(|i| i.state == State::Running).map(|i| i.speed).sum();
                let mut text = match infos.len() {
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

fn open_file(window: &gtk::ApplicationWindow, path: PathBuf) {
    let launcher = gtk::FileLauncher::new(Some(&gio::File::for_path(&path)));
    let parent = window.clone().upcast::<gtk::Window>();
    launcher.launch(Some(window), gio::Cancellable::NONE, move |result| {
        if let Err(e) = result {
            show_error(Some(&parent), "Não foi possível abrir o arquivo", &e.to_string());
        }
    });
}

fn open_folder(window: &gtk::ApplicationWindow, info: &DownloadInfo) {
    let parent = window.clone().upcast::<gtk::Window>();
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
    use super::looks_like_url;

    #[test]
    fn clipboard_url_detection() {
        assert!(looks_like_url("https://example.com/a.iso"));
        assert!(looks_like_url("  ftp://example.com/a  "));
        assert!(!looks_like_url("example.com"));
        assert!(!looks_like_url("https://a b"));
    }
}
