//! "Preferências". Geral: download folder, one subfolder per category,
//! connections per file, the progress window, the speed limit, the clipboard
//! monitor, the tray icon and session autostart. Filas: the queues, each with
//! its downloads at the same time, its schedule and what to do when it is
//! done. Navegador: the file types the browser hands to Fisga and the key that
//! sends a link to it. Aparência: the theme and the holidays' themes. The settings live in the daemon (`~/.config/fisga/settings.json`);
//! autostart is a file in `~/.config/autostart`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use fisga_ipc::{
    BrowserSettings, Category, ForceKey, ProxyMode, ProxySettings, Queue, Request, Schedule, Settings, SiteLogin, Theme,
    TimeOfDay, WhenDone, MAIN_QUEUE, WEEKDAYS,
};

use crate::festive::Season;
use crate::window::show_error;
use crate::{autostart, ipc, theme};

fn row(grid: &gtk::Grid, index: i32, caption: &str, widget: &impl IsA<gtk::Widget>, hint: Option<&str>) {
    // A minimum width: wide controls (the weekday buttons) must not squeeze the text.
    let label = gtk::Label::builder().label(caption).xalign(0.0).hexpand(true).wrap(true).width_chars(24).build();
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.append(&label);
    if let Some(hint) = hint {
        let hint = gtk::Label::builder().label(hint).xalign(0.0).wrap(true).width_chars(34).max_width_chars(48).build();
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        text.append(&hint);
    }
    grid.attach(&text, 0, index, 1, 1);
    widget.set_valign(gtk::Align::Center);
    widget.set_halign(gtk::Align::End);
    grid.attach(widget, 1, index, 1, 1);
}

fn folder_text(dir: &Option<PathBuf>) -> String {
    match dir {
        Some(dir) => dir.display().to_string(),
        None => "Downloads do sistema".into(),
    }
}

fn time_entry(time: Option<TimeOfDay>) -> gtk::Entry {
    let text = time.map(|t| t.to_string()).unwrap_or_default();
    gtk::Entry::builder().text(text).placeholder_text("HH:MM").width_chars(6).max_width_chars(6).build()
}

fn parse_time(entry: &gtk::Entry, what: &str) -> Result<TimeOfDay, String> {
    TimeOfDay::parse(&entry.text()).ok_or_else(|| format!("{what}: use HH:MM, como 02:00 ou 23:30."))
}

/// The tab to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    General = 0,
    Queues = 1,
    Network = 2,
}

/// One site login in the "Rede" tab.
struct LoginRow {
    row: gtk::Box,
    host: gtk::Entry,
    user: gtk::Entry,
    password: gtk::PasswordEntry,
}

/// The "Rede" tab: the proxy and the site logins. Returns the tab, a function
/// that reads the proxy back, and one that reads the logins back (`None`
/// until they arrived from the daemon, so saving never wipes them).
fn network_page(proxy: &ProxySettings) -> (gtk::Box, ReadProxy, ReadLogins) {
    const MODES: [ProxyMode; 3] = [ProxyMode::System, ProxyMode::None, ProxyMode::Manual];
    let mode = gtk::DropDown::from_strings(&["Do sistema", "Sem proxy", "Manual"]);
    mode.set_selected(MODES.iter().position(|m| *m == proxy.mode).unwrap_or(0) as u32);
    let url = gtk::Entry::builder().text(&proxy.url).placeholder_text("http://host:3128").width_chars(30).build();
    url.set_sensitive(proxy.mode == ProxyMode::Manual);
    let u = url.clone();
    mode.connect_selected_notify(move |mode| u.set_sensitive(MODES.get(mode.selected() as usize) == Some(&ProxyMode::Manual)));

    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(
        &grid,
        0,
        "Proxy",
        &mode,
        Some("\u{201c}Do sistema\u{201d} usa as variáveis http_proxy e https_proxy. Vale para todos os downloads, inclusive os vídeos de sites."),
    );
    row(
        &grid,
        1,
        "Endereço do proxy",
        &url,
        Some("http://host:porta ou socks5://host:porta; com senha: socks5://usuário:senha@host:porta."),
    );

    let logins_title = gtk::Label::builder().label("Logins por site").xalign(0.0).build();
    logins_title.add_css_class("heading");
    let logins_hint = gtk::Label::builder()
        .label("Usuário e senha enviados aos links diretos do site, só por https (autenticação HTTP). Para valer também nos subdomínios, use *.exemplo.com. Ficam só no seu usuário, fora das configurações.")
        .xalign(0.0)
        .wrap(true)
        .width_chars(34)
        .max_width_chars(60)
        .build();
    logins_hint.add_css_class("dim-label");
    logins_hint.add_css_class("caption");
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let loading = gtk::Label::builder().label("Carregando…").xalign(0.0).build();
    list.append(&loading);
    let add = gtk::Button::builder().label("Adicionar login").halign(gtk::Align::Start).sensitive(false).build();

    let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    page.append(&grid);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    page.append(&logins_title);
    page.append(&logins_hint);
    page.append(&list);
    page.append(&add);

    let rows: Rc<RefCell<Vec<Rc<LoginRow>>>> = Rc::default();
    let loaded = Rc::new(Cell::new(false));
    let add_row = {
        let (rows, list) = (rows.clone(), list.clone());
        Rc::new(move |login: &SiteLogin| {
            let host = gtk::Entry::builder().text(&login.host).placeholder_text("arquivos.exemplo.com").hexpand(true).build();
            let user = gtk::Entry::builder().text(&login.user).placeholder_text("usuário").width_chars(12).build();
            let password = gtk::PasswordEntry::builder().show_peek_icon(true).build();
            password.set_text(&login.password);
            password.set_width_chars(12);
            password.set_placeholder_text(Some("senha"));
            let remove = gtk::Button::builder().icon_name("user-trash-symbolic").tooltip_text("Esquecer este login").build();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(&host);
            row.append(&user);
            row.append(&password);
            row.append(&remove);
            list.append(&row);
            let entry = Rc::new(LoginRow { row, host, user, password });
            rows.borrow_mut().push(entry.clone());
            let (rows, list) = (rows.clone(), list.clone());
            remove.connect_clicked(move |_| {
                list.remove(&entry.row);
                rows.borrow_mut().retain(|other| !Rc::ptr_eq(other, &entry));
            });
        })
    };
    let a = add_row.clone();
    add.connect_clicked(move |_| a(&SiteLogin { host: String::new(), user: String::new(), password: String::new() }));
    let (l, a, add_button) = (loaded.clone(), add_row.clone(), add.clone());
    glib::spawn_future_local(async move {
        match ipc::call(Request::GetLogins).await {
            Ok(fisga_ipc::Response::Logins { logins }) => {
                for login in &logins {
                    a(login);
                }
                loading.set_visible(false);
                l.set(true);
                add_button.set_sensitive(true);
            }
            _ => loading.set_label("Não foi possível ler os logins do fisgad."),
        }
    });

    let read_proxy = Box::new(move || {
        let proxy = ProxySettings {
            mode: MODES.get(mode.selected() as usize).copied().unwrap_or_default(),
            url: url.text().trim().to_string(),
        };
        proxy.check().map(|_| proxy)
    });
    let read_logins = Box::new(move || {
        loaded.get().then(|| {
            rows.borrow()
                .iter()
                .map(|r| SiteLogin { host: r.host.text().trim().to_string(), user: r.user.text().to_string(), password: r.password.text().to_string() })
                .filter(|login| !login.host.is_empty())
                .collect()
        })
    });
    (page, read_proxy, read_logins)
}

type ReadProxy = Box<dyn Fn() -> Result<ProxySettings, String>>;
type ReadLogins = Box<dyn Fn() -> Option<Vec<SiteLogin>>>;

/// The "Navegador" tab (IDM's "Tipos de arquivo" and "Teclas"): what the
/// browser extension hands to Fisga. Returns the tab and a function that reads
/// it back.
fn browser_page(browser: &BrowserSettings) -> (gtk::Box, Box<dyn Fn() -> BrowserSettings>) {
    let types = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::Word)
        .accepts_tab(false)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    types.buffer().set_text(&browser.file_types.join(" "));
    let types_frame = gtk::Frame::builder().child(&types).height_request(110).build();
    let reset = gtk::Button::builder().label("Restaurar a lista padrão").halign(gtk::Align::Start).build();
    let t = types.clone();
    reset.connect_clicked(move |_| t.buffer().set_text(&BrowserSettings::default().file_types.join(" ")));

    let types_title = gtk::Label::builder().label("Tipos de arquivo").xalign(0.0).build();
    types_title.add_css_class("heading");
    let types_hint = gtk::Label::builder()
        .label("Os downloads do navegador com estas extensões vão para o Fisga. Separe com espaços, sem ponto (safetensors, gguf, pdf...). Vídeos, músicas e arquivos compactados também são reconhecidos pelo tipo que o site informa.")
        .xalign(0.0)
        .wrap(true)
        .width_chars(34)
        .max_width_chars(60)
        .build();
    types_hint.add_css_class("dim-label");
    types_hint.add_css_class("caption");

    let labels: Vec<&str> = ForceKey::ALL.iter().map(|key| key.label()).collect();
    let key = gtk::DropDown::from_strings(&labels);
    key.set_selected(ForceKey::ALL.iter().position(|k| *k == browser.force_key).unwrap_or(0) as u32);
    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(
        &grid,
        0,
        "Tecla para baixar com o Fisga",
        &key,
        Some("Segure ao clicar num link: ele vai para o Fisga, seja qual for o tipo de arquivo. Para os casos que escapam da lista acima."),
    );

    let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    page.append(&types_title);
    page.append(&types_hint);
    page.append(&types_frame);
    page.append(&reset);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    page.append(&grid);
    let note = gtk::Label::builder()
        .label("Vale para a extensão do Fisga no navegador; ela recebe as mudanças em alguns segundos.")
        .xalign(0.0)
        .wrap(true)
        .width_chars(34)
        .max_width_chars(60)
        .build();
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    page.append(&note);

    let read = move || {
        let buffer = types.buffer();
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
        BrowserSettings {
            file_types: BrowserSettings::parse_file_types(&text),
            force_key: ForceKey::ALL.get(key.selected() as usize).copied().unwrap_or_default(),
        }
    };
    (page, Box::new(read))
}

/// The "Aparência" tab: the theme, shown as soon as it is picked, and the
/// holidays' themes, any of which can be looked at. Returns the tab and a
/// function that reads it back.
fn look_page(settings: &Settings) -> (gtk::Grid, Box<dyn Fn() -> (Theme, bool)>) {
    const THEMES: [Theme; 2] = [Theme::Dark, Theme::Light];
    let theme_choice = gtk::DropDown::from_strings(&["Escuro", "Claro"]);
    theme_choice.set_selected(THEMES.iter().position(|t| *t == settings.theme).unwrap_or(0) as u32);
    theme_choice.connect_selected_notify(|choice| theme::set_light(THEMES.get(choice.selected() as usize) == Some(&Theme::Light)));
    let festive = gtk::Switch::builder().active(settings.festive).build();

    // A holiday's theme, shown at once and until the window closes.
    let labels: Vec<&str> = std::iter::once("Nenhuma").chain(Season::ALL.iter().map(|season| season.label())).collect();
    let season = gtk::DropDown::from_strings(&labels);
    season.connect_selected_notify(|choice| {
        let picked = (choice.selected() as usize).checked_sub(1).and_then(|i| Season::ALL.get(i)).copied();
        theme::preview(picked);
    });

    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(&grid, 0, "Tema", &theme_choice, Some("Na semana de uma data comemorativa, vale o tema da data."));
    row(
        &grid,
        1,
        "Temas de datas comemorativas",
        &festive,
        Some(
            "Na semana da data, a janela principal ganha o fundo e as cores dela, com uma animação atrás dos downloads: \
             confete no Carnaval, ovos na Páscoa, um coração neon no Dia dos Namorados, bandeirinhas na festa junina, \
             morcegos no Halloween, neve e pisca-pisca no Natal e fogos no Ano-novo. A animação para com a janela minimizada.",
        ),
    );
    row(&grid, 2, "Ver o tema de uma data", &season, Some("Enquanto as Preferências estiverem abertas."));
    let read = move || (THEMES.get(theme_choice.selected() as usize).copied().unwrap_or_default(), festive.is_active());
    (grid, Box::new(read))
}

/// The settings of one queue, in the "Filas" tab.
struct QueueEditor {
    name: gtk::Entry,
    parallel: gtk::SpinButton,
    enabled: gtk::Switch,
    start: gtk::Entry,
    has_stop: gtk::CheckButton,
    stop: gtk::Entry,
    days: Vec<gtk::ToggleButton>,
    when_done: gtk::DropDown,
    /// Shown details depend on the scheduler switch.
    details: Vec<gtk::Widget>,
}

impl QueueEditor {
    fn new() -> (Self, gtk::Grid) {
        let name = gtk::Entry::builder().width_chars(20).build();
        let parallel = gtk::SpinButton::with_range(1.0, 20.0, 1.0);
        let enabled = gtk::Switch::new();
        let start = time_entry(None);
        let has_stop = gtk::CheckButton::new();
        let stop = time_entry(None);
        let s = stop.clone();
        has_stop.connect_toggled(move |check| s.set_sensitive(check.is_active()));
        let stop_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        stop_box.append(&has_stop);
        stop_box.append(&stop);
        let days_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        days_box.add_css_class("linked");
        let days: Vec<gtk::ToggleButton> = WEEKDAYS
            .iter()
            .map(|name| {
                let mut label = name.to_string();
                label[..1].make_ascii_uppercase();
                let button = gtk::ToggleButton::with_label(&label);
                days_box.append(&button);
                button
            })
            .collect();
        let labels: Vec<&str> = WhenDone::ALL.iter().map(|w| w.label()).collect();
        let when_done = gtk::DropDown::from_strings(&labels);

        let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
        row(&grid, 0, "Nome", &name, None);
        row(&grid, 1, "Downloads ao mesmo tempo", &parallel, Some("Os outros desta fila esperam a vez."));
        row(
            &grid,
            2,
            "Usar o agendador",
            &enabled,
            Some("Os downloads desta fila só rodam no horário abaixo. \u{201c}Baixar agora\u{201d} e retomar um download à mão continuam valendo na hora."),
        );
        row(&grid, 3, "Começar às", &start, None);
        row(
            &grid,
            4,
            "Parar às",
            &stop_box,
            Some("No horário de parada, os downloads voltam para a fila. Sem horário, a fila vai até terminar."),
        );
        row(&grid, 5, "Dias", &days_box, None);
        row(
            &grid,
            6,
            "Ao terminar a fila",
            &when_done,
            Some("Antes de suspender ou desligar, uma janela conta 60 segundos e deixa cancelar."),
        );
        let details: Vec<gtk::Widget> =
            vec![start.clone().upcast(), stop_box.upcast(), days_box.upcast(), when_done.clone().upcast()];
        let d = details.clone();
        enabled.connect_active_notify(move |switch| d.iter().for_each(|w| w.set_sensitive(switch.is_active())));
        let editor = Self { name, parallel, enabled, start, has_stop, stop, days, when_done, details };
        (editor, grid)
    }

    fn load(&self, queue: &Queue) {
        self.name.set_text(&queue.name);
        // The main queue keeps its name.
        self.name.set_sensitive(queue.id != MAIN_QUEUE);
        self.parallel.set_value(queue.max_parallel as f64);
        let schedule = &queue.schedule;
        self.enabled.set_active(schedule.enabled);
        self.start.set_text(&schedule.start.to_string());
        self.has_stop.set_active(schedule.stop.is_some());
        self.stop.set_text(&schedule.stop.map(|t| t.to_string()).unwrap_or_default());
        self.stop.set_sensitive(schedule.stop.is_some());
        for (i, button) in self.days.iter().enumerate() {
            button.set_active(schedule.days.contains(&(i as u8)));
        }
        let when_done = WhenDone::ALL.iter().position(|w| *w == schedule.when_done).unwrap_or(0);
        self.when_done.set_selected(when_done as u32);
        self.details.iter().for_each(|w| w.set_sensitive(schedule.enabled));
    }

    /// The edited queue, or what to fix.
    fn read(&self, base: &Queue) -> Result<Queue, String> {
        let start = parse_time(&self.start, "Começar às")?;
        let stop = match self.has_stop.is_active() {
            true => Some(parse_time(&self.stop, "Parar às")?),
            false => None,
        };
        let days: Vec<u8> = self.days.iter().enumerate().filter(|(_, b)| b.is_active()).map(|(i, _)| i as u8).collect();
        if self.enabled.is_active() && days.is_empty() {
            return Err("Escolha pelo menos um dia para o agendador.".into());
        }
        let when_done = WhenDone::ALL.get(self.when_done.selected() as usize).copied().unwrap_or_default();
        let name = match base.id {
            MAIN_QUEUE => base.name.clone(),
            _ => self.name.text().trim().to_string(),
        };
        if name.is_empty() {
            return Err("Dê um nome à fila.".into());
        }
        Ok(Queue {
            id: base.id,
            name,
            max_parallel: self.parallel.value() as usize,
            schedule: Schedule { enabled: self.enabled.is_active(), start, stop, days, when_done },
        })
    }
}

/// Reads the edited queues back, or says what to fix.
type ReadQueues = Box<dyn Fn() -> Result<Vec<Queue>, String>>;

/// The "Filas" tab (IDM's scheduler window): pick a queue, create or remove
/// one, and edit it.
fn queues_page(parent: &gtk::Window, queues: Vec<Queue>, next_id: u64) -> (gtk::Box, ReadQueues) {
    let queues = Rc::new(RefCell::new(queues));
    let current = Rc::new(Cell::new(0usize));
    let next_id = Rc::new(Cell::new(next_id));
    // Set while the list is rebuilt, so `connect_selected_notify` does not react.
    let rebuilding = Rc::new(Cell::new(false));
    let (editor, grid) = QueueEditor::new();
    let editor = Rc::new(editor);

    let choice = gtk::DropDown::builder().hexpand(true).build();
    let add = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Nova fila").build();
    let remove = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Excluir a fila (os downloads dela vão para a fila principal)")
        .build();
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.append(&gtk::Label::new(Some("Fila")));
    bar.append(&choice);
    bar.append(&add);
    bar.append(&remove);
    let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.append(&bar);
    page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    page.append(&grid);

    // Shows the list of names and the queue at `index`.
    let show = {
        let (queues, current, rebuilding, editor, choice, remove) =
            (queues.clone(), current.clone(), rebuilding.clone(), editor.clone(), choice.clone(), remove.clone());
        Rc::new(move |index: usize| {
            let list = queues.borrow();
            let names: Vec<&str> = list.iter().map(|q| q.name.as_str()).collect();
            rebuilding.set(true);
            choice.set_model(Some(&gtk::StringList::new(&names)));
            choice.set_selected(index as u32);
            rebuilding.set(false);
            current.set(index);
            editor.load(&list[index]);
            remove.set_sensitive(list[index].id != MAIN_QUEUE);
        })
    };
    // Keeps the edits of the shown queue; false (and an error message) when they are invalid.
    let store = {
        let (queues, current, editor, parent) = (queues.clone(), current.clone(), editor.clone(), parent.clone());
        Rc::new(move || {
            let index = current.get();
            let edited = editor.read(&queues.borrow()[index]);
            match edited {
                Ok(queue) => {
                    queues.borrow_mut()[index] = queue;
                    true
                }
                Err(message) => {
                    show_error(Some(&parent), "Confira a fila", &message);
                    false
                }
            }
        })
    };
    show(0);

    let (s, st, r, c) = (show.clone(), store.clone(), rebuilding.clone(), current.clone());
    choice.connect_selected_notify(move |choice| {
        if r.get() {
            return;
        }
        let wanted = choice.selected() as usize;
        // Invalid edits keep the queue on screen.
        let index = if st() { wanted } else { c.get() };
        s(index);
    });
    let (s, st, q, n) = (show.clone(), store.clone(), queues.clone(), next_id.clone());
    add.connect_clicked(move |_| {
        if !st() {
            return;
        }
        let id = n.get();
        n.set(id + 1);
        q.borrow_mut().push(Queue { id, name: format!("Fila {id}"), max_parallel: 1, ..Queue::default() });
        let last = q.borrow().len() - 1;
        s(last);
    });
    let (s, q, c) = (show.clone(), queues.clone(), current.clone());
    remove.connect_clicked(move |_| {
        let index = c.get();
        if index > 0 {
            q.borrow_mut().remove(index);
            s(0);
        }
    });

    let read = move || {
        let index = current.get();
        let queue = editor.read(&queues.borrow()[index])?;
        queues.borrow_mut()[index] = queue;
        Ok(queues.borrow().clone())
    };
    (page, Box::new(read))
}

pub fn show(parent: &gtk::Window, settings: Settings, page: Page) {
    let window = gtk::Window::builder()
        .title("Preferências")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .resizable(false)
        .build();

    let dir = Rc::new(RefCell::new(settings.download_dir.clone()));
    let dir_label = gtk::Label::builder()
        .label(folder_text(&settings.download_dir))
        .ellipsize(pango::EllipsizeMode::Start)
        .max_width_chars(28)
        .xalign(1.0)
        .build();
    let choose = gtk::Button::with_label("Alterar…");
    let reset = gtk::Button::builder().label("Padrão").tooltip_text("Usar a pasta de Downloads do sistema").build();
    let dir_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    dir_box.append(&dir_label);
    dir_box.append(&choose);
    dir_box.append(&reset);

    let folders = gtk::Switch::builder().active(settings.category_folders).build();
    let connections = gtk::SpinButton::with_range(1.0, 32.0, 1.0);
    connections.set_value(settings.connections as f64);
    let progress = gtk::Switch::builder().active(settings.show_progress).build();
    let limit = gtk::SpinButton::with_range(0.0, 1_000_000.0, 64.0);
    limit.set_value((settings.speed_limit / 1024) as f64);
    let clipboard = gtk::Switch::builder().active(settings.watch_clipboard).build();
    let tray = gtk::Switch::builder().active(settings.tray).build();
    let autostart_was = autostart::is_enabled();
    let session = gtk::Switch::builder().active(autostart_was).build();

    let subfolders: Vec<&str> = Category::ALL.iter().filter_map(|c| c.folder()).collect();
    let subfolders_hint = format!("Subpastas: {}. Os demais ficam na pasta de downloads.", subfolders.join(", "));
    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(&grid, 0, "Pasta de downloads", &dir_box, None);
    row(&grid, 1, "Separar por tipo de arquivo", &folders, Some(subfolders_hint.as_str()));
    row(&grid, 2, "Conexões por arquivo", &connections, Some("Para links diretos; vídeos de sites usam o yt-dlp."));
    row(&grid, 3, "Mostrar a janela de progresso", &progress, Some("Ao clicar em \u{201c}Baixar agora\u{201d}."));
    row(
        &grid,
        4,
        "Limite de velocidade (KB/s)",
        &limit,
        Some("Todos os downloads juntos; 0 = sem limite. Vale na hora, até para os que estão baixando."),
    );
    row(
        &grid,
        5,
        "Capturar links copiados",
        &clipboard,
        Some(
            "Ao copiar o endereço de um vídeo, música, programa, arquivo compactado ou PDF, abre a janela \u{201c}Novo \
             download\u{201d}. No Wayland, precisa do wl-clipboard.",
        ),
    );
    row(
        &grid,
        6,
        "Ícone na bandeja",
        &tray,
        Some("Fechar a janela deixa o Fisga na bandeja, pronto para os downloads do navegador. Precisa de uma bandeja (waybar, noctalia, KDE...)."),
    );
    row(&grid, 7, "Iniciar com a sessão", &session, Some("Abre o Fisga na bandeja, sem janela, ao entrar no sistema."));

    let (queues_box, read_queues) = queues_page(window.upcast_ref(), settings.all_queues(), settings.new_queue_id());
    let (network_box, read_proxy, read_logins) = network_page(&settings.proxy);
    let (browser_box, read_browser) = browser_page(&settings.browser);
    let (look_box, read_look) = look_page(&settings);

    // "Ao concluir".
    let done = &settings.on_complete;
    let sound = gtk::Switch::builder().active(done.sound).build();
    let antivirus = gtk::Switch::builder().active(done.antivirus).build();
    let command = gtk::Entry::builder()
        .text(&done.command)
        .placeholder_text("mv \"$FISGA_FILE\" ~/Concluídos/")
        .width_chars(30)
        .build();
    let done_grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(&done_grid, 0, "Tocar um som", &sound, Some("O som de \u{201c}concluído\u{201d} do sistema."));
    row(
        &done_grid,
        1,
        "Verificar com o antivírus",
        &antivirus,
        Some("Usa o ClamAV (pacote clamav). Se achar um vírus, avisa, marca o download e não roda o comando abaixo."),
    );
    row(
        &done_grid,
        2,
        "Rodar um comando",
        &command,
        Some("No shell, com o arquivo em $FISGA_FILE (e $FISGA_NAME, $FISGA_URL, $FISGA_DIR). Use as variáveis entre aspas. Vazio = nada."),
    );

    let notebook = gtk::Notebook::new();
    let pages: [(&gtk::Widget, &str); 6] = [
        (grid.upcast_ref(), "Geral"),
        (queues_box.upcast_ref(), "Filas"),
        (network_box.upcast_ref(), "Rede"),
        (done_grid.upcast_ref(), "Ao concluir"),
        (browser_box.upcast_ref(), "Navegador"),
        (look_box.upcast_ref(), "Aparência"),
    ];
    for (page, title) in pages {
        page.set_margin_top(14);
        page.set_margin_bottom(6);
        page.set_margin_start(6);
        page.set_margin_end(12);
        // Scrolls instead of growing past small screens (the buttons must stay visible).
        let scroll = gtk::ScrolledWindow::builder()
            .child(page)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .max_content_height(560)
            .build();
        notebook.append_page(&scroll, Some(&gtk::Label::new(Some(title))));
    }
    notebook.set_current_page(Some(page as u32));

    let cancel = gtk::Button::with_label("Cancelar");
    let save = gtk::Button::with_label("Salvar");
    save.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).margin_top(6).build();
    buttons.append(&cancel);
    buttons.append(&save);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(&notebook);
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&save));

    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    crate::window::close_on_escape(&window);
    // The theme picked shows at once; closed without saving, the saved one is back.
    // A holiday's theme shown from "Aparência" ends with the window too. On
    // closing, not on "destroy": the closures here that hold the window keep
    // it from ever being destroyed.
    let saved = Rc::new(Cell::new(false));
    let (sv, light) = (saved.clone(), settings.theme == Theme::Light);
    window.connect_close_request(move |_| {
        theme::preview(None);
        if !sv.get() {
            theme::set_light(light);
        }
        glib::Propagation::Proceed
    });

    let (w, d, l) = (window.clone(), dir.clone(), dir_label.clone());
    choose.connect_clicked(move |_| {
        let (w, d, l) = (w.clone(), d.clone(), l.clone());
        glib::spawn_future_local(async move {
            let mut dialog = gtk::FileDialog::builder().title("Pasta de downloads").modal(true);
            if let Some(current) = d.borrow().as_ref() {
                dialog = dialog.initial_folder(&gio::File::for_path(current));
            }
            if let Ok(folder) = dialog.build().select_folder_future(Some(&w)).await {
                if let Some(path) = folder.path() {
                    l.set_label(&folder_text(&Some(path.clone())));
                    d.replace(Some(path));
                }
            }
        });
    });
    let (d, l) = (dir.clone(), dir_label.clone());
    reset.connect_clicked(move |_| {
        d.replace(None);
        l.set_label(&folder_text(&None));
    });

    let w = window.clone();
    save.connect_clicked(move |button| {
        let queues = match read_queues() {
            Ok(queues) => queues,
            Err(message) => {
                notebook.set_current_page(Some(Page::Queues as u32));
                show_error(Some(&w), "Confira a fila", &message);
                return;
            }
        };
        let proxy = match read_proxy() {
            Ok(proxy) => proxy,
            Err(message) => {
                notebook.set_current_page(Some(Page::Network as u32));
                show_error(Some(&w), "Confira o proxy", &message);
                return;
            }
        };
        let logins = read_logins();
        let (theme, festive) = read_look();
        let on_complete = fisga_ipc::OnComplete {
            sound: sound.is_active(),
            antivirus: antivirus.is_active(),
            command: command.text().trim().to_string(),
        };
        let mut new = Settings {
            download_dir: dir.borrow().clone(),
            category_folders: folders.is_active(),
            max_parallel: settings.max_parallel,
            connections: connections.value() as usize,
            show_progress: progress.is_active(),
            speed_limit: limit.value() as u64 * 1024,
            watch_clipboard: clipboard.is_active(),
            tray: tray.is_active(),
            schedule: settings.schedule.clone(),
            queues: Vec::new(),
            last_queue_id: settings.last_queue_id,
            proxy,
            hidden_columns: settings.hidden_columns.clone(),
            list_view: settings.list_view,
            newest_first: settings.newest_first,
            on_complete,
            browser: read_browser(),
            theme,
            festive,
        };
        for queue in queues {
            new.set_queue(queue);
        }
        if session.is_active() != autostart_was {
            if let Err(e) = autostart::set_enabled(session.is_active()) {
                show_error(Some(&w), "Não foi possível mudar o início com a sessão", &e.to_string());
                return;
            }
        }
        button.set_sensitive(false);
        let (w, button, done) = (w.clone(), button.clone(), saved.clone());
        glib::spawn_future_local(async move {
            let saved = match ipc::call(Request::SetSettings { settings: new }).await {
                Ok(_) => match logins {
                    Some(logins) => ipc::call(Request::SetLogins { logins }).await,
                    None => Ok(fisga_ipc::Response::Ok),
                },
                Err(e) => Err(e),
            };
            match saved {
                Ok(_) => {
                    done.set(true);
                    w.close();
                }
                Err(e) => {
                    show_error(Some(&w), "Não foi possível salvar", &e.to_string());
                    button.set_sensitive(true);
                }
            }
        });
    });
    window.present();
}
