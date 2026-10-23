//! "Preferências". Geral: download folder, one subfolder per category,
//! downloads at the same time, connections per file, the progress window, the
//! speed limit, the clipboard monitor, the tray icon and session autostart.
//! Agendador: when the queue runs and what to do when it is done. The settings
//! live in the daemon (`~/.config/ldm/settings.json`); autostart is a file in
//! `~/.config/autostart`.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use ldm_ipc::{Category, Request, Schedule, Settings, TimeOfDay, WhenDone, WEEKDAYS};

use crate::window::show_error;
use crate::{autostart, ipc};

fn row(grid: &gtk::Grid, index: i32, caption: &str, widget: &impl IsA<gtk::Widget>, hint: Option<&str>) {
    let label = gtk::Label::builder().label(caption).xalign(0.0).hexpand(true).wrap(true).build();
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.append(&label);
    if let Some(hint) = hint {
        let hint = gtk::Label::builder().label(hint).xalign(0.0).wrap(true).max_width_chars(48).build();
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

/// The "Agendador" tab and a function that reads it back.
fn schedule_page(schedule: &Schedule) -> (gtk::Grid, Box<dyn Fn() -> Result<Schedule, String>>) {
    let enabled = gtk::Switch::builder().active(schedule.enabled).build();
    let start = time_entry(Some(schedule.start));
    let has_stop = gtk::CheckButton::builder().active(schedule.stop.is_some()).build();
    let stop = time_entry(schedule.stop);
    stop.set_sensitive(schedule.stop.is_some());
    let s = stop.clone();
    has_stop.connect_toggled(move |check| s.set_sensitive(check.is_active()));
    let stop_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    stop_box.append(&has_stop);
    stop_box.append(&stop);

    let days_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    days_box.add_css_class("linked");
    let days: Vec<gtk::ToggleButton> = WEEKDAYS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let mut label = name.to_string();
            label[..1].make_ascii_uppercase();
            let button = gtk::ToggleButton::builder().label(label).active(schedule.days.contains(&(i as u8))).build();
            days_box.append(&button);
            button
        })
        .collect();

    let labels: Vec<&str> = WhenDone::ALL.iter().map(|w| w.label()).collect();
    let when_done = gtk::DropDown::from_strings(&labels);
    when_done.set_selected(WhenDone::ALL.iter().position(|w| *w == schedule.when_done).unwrap_or(0) as u32);

    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(
        &grid,
        0,
        "Usar o agendador",
        &enabled,
        Some("Os downloads da fila só rodam no horário abaixo. \u{201c}Baixar agora\u{201d} e retomar um download à mão continuam valendo na hora."),
    );
    row(&grid, 1, "Começar às", &start, None);
    row(
        &grid,
        2,
        "Parar às",
        &stop_box,
        Some("No horário de parada, os downloads voltam para a fila. Sem horário, a fila vai até terminar."),
    );
    row(&grid, 3, "Dias", &days_box, None);
    row(
        &grid,
        4,
        "Ao terminar a fila",
        &when_done,
        Some("Antes de suspender ou desligar, uma janela conta 60 segundos e deixa cancelar."),
    );
    let details = [start.upcast_ref::<gtk::Widget>(), stop_box.upcast_ref(), days_box.upcast_ref(), when_done.upcast_ref()]
        .map(|w| w.clone());
    let sync = move |on: bool| details.iter().for_each(|w| w.set_sensitive(on));
    sync(schedule.enabled);
    enabled.connect_active_notify(move |switch| sync(switch.is_active()));

    let read = move || {
        let start = parse_time(&start, "Começar às")?;
        let stop = match has_stop.is_active() {
            true => Some(parse_time(&stop, "Parar às")?),
            false => None,
        };
        let days: Vec<u8> = days.iter().enumerate().filter(|(_, b)| b.is_active()).map(|(i, _)| i as u8).collect();
        if enabled.is_active() && days.is_empty() {
            return Err("Escolha pelo menos um dia para o agendador.".into());
        }
        let when_done = WhenDone::ALL.get(when_done.selected() as usize).copied().unwrap_or_default();
        Ok(Schedule { enabled: enabled.is_active(), start, stop, days, when_done })
    };
    (grid, Box::new(read))
}

pub fn show(parent: &gtk::Window, settings: Settings) {
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
    let parallel = gtk::SpinButton::with_range(1.0, 20.0, 1.0);
    parallel.set_value(settings.max_parallel as f64);
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
    row(&grid, 2, "Downloads ao mesmo tempo", &parallel, Some("Os outros esperam na fila."));
    row(&grid, 3, "Conexões por arquivo", &connections, Some("Para links diretos; vídeos de sites usam o yt-dlp."));
    row(&grid, 4, "Mostrar a janela de progresso", &progress, Some("Ao clicar em \u{201c}Baixar agora\u{201d}."));
    row(
        &grid,
        5,
        "Limite de velocidade (KB/s)",
        &limit,
        Some("Todos os downloads juntos; 0 = sem limite. Vale na hora, até para os que estão baixando."),
    );
    row(
        &grid,
        6,
        "Capturar links copiados",
        &clipboard,
        Some(
            "Ao copiar o endereço de um vídeo, música, programa, arquivo compactado ou PDF, abre a janela \u{201c}Novo \
             download\u{201d}. No Wayland, precisa do wl-clipboard.",
        ),
    );
    row(
        &grid,
        7,
        "Ícone na bandeja",
        &tray,
        Some("Fechar a janela deixa o LDM na bandeja, pronto para os downloads do navegador. Precisa de uma bandeja (waybar, noctalia, KDE...)."),
    );
    row(&grid, 8, "Iniciar com a sessão", &session, Some("Abre o LDM na bandeja, sem janela, ao entrar no sistema."));

    let (schedule_grid, read_schedule) = schedule_page(&settings.schedule);
    let notebook = gtk::Notebook::new();
    for (page, title) in [(&grid, "Geral"), (&schedule_grid, "Agendador")] {
        page.set_margin_top(14);
        page.set_margin_bottom(6);
        page.set_margin_start(6);
        page.set_margin_end(6);
        notebook.append_page(page, Some(&gtk::Label::new(Some(title))));
    }

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
        let schedule = match read_schedule() {
            Ok(schedule) => schedule,
            Err(message) => {
                notebook.set_current_page(Some(1));
                show_error(Some(&w), "Confira o agendador", &message);
                return;
            }
        };
        let new = Settings {
            download_dir: dir.borrow().clone(),
            category_folders: folders.is_active(),
            max_parallel: parallel.value() as usize,
            connections: connections.value() as usize,
            show_progress: progress.is_active(),
            speed_limit: limit.value() as u64 * 1024,
            watch_clipboard: clipboard.is_active(),
            tray: tray.is_active(),
            schedule,
        };
        if session.is_active() != autostart_was {
            if let Err(e) = autostart::set_enabled(session.is_active()) {
                show_error(Some(&w), "Não foi possível mudar o início com a sessão", &e.to_string());
                return;
            }
        }
        button.set_sensitive(false);
        let (w, button) = (w.clone(), button.clone());
        glib::spawn_future_local(async move {
            match ipc::call(Request::SetSettings { settings: new }).await {
                Ok(_) => w.close(),
                Err(e) => {
                    show_error(Some(&w), "Não foi possível salvar", &e.to_string());
                    button.set_sensitive(true);
                }
            }
        });
    });
    window.present();
}
