//! "Preferências": download folder, one subfolder per category, downloads at
//! the same time, connections per file and the progress window. The settings
//! live in the daemon (`~/.config/ldm/settings.json`).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib, pango};
use ldm_ipc::{Category, Request, Settings};

use crate::ipc;
use crate::window::show_error;

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
    grid.attach(widget, 1, index, 1, 1);
}

fn folder_text(dir: &Option<PathBuf>) -> String {
    match dir {
        Some(dir) => dir.display().to_string(),
        None => "Pasta de Downloads do sistema".into(),
    }
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
    let reset = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Usar a pasta padrão").build();
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

    let subfolders: Vec<&str> = Category::ALL.iter().filter_map(|c| c.folder()).collect();
    let subfolders_hint = format!("Subpastas: {}. Os demais ficam na pasta de downloads.", subfolders.join(", "));
    let grid = gtk::Grid::builder().column_spacing(18).row_spacing(14).build();
    row(&grid, 0, "Pasta de downloads", &dir_box, None);
    row(&grid, 1, "Separar por tipo de arquivo", &folders, Some(subfolders_hint.as_str()));
    row(&grid, 2, "Downloads ao mesmo tempo", &parallel, Some("Os outros esperam na fila."));
    row(&grid, 3, "Conexões por arquivo", &connections, Some("Para links diretos; vídeos de sites usam o yt-dlp."));
    row(&grid, 4, "Mostrar a janela de progresso", &progress, Some("Ao clicar em \u{201c}Baixar agora\u{201d}."));

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
    content.append(&grid);
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
        let new = Settings {
            download_dir: dir.borrow().clone(),
            category_folders: folders.is_active(),
            max_parallel: parallel.value() as usize,
            connections: connections.value() as usize,
            show_progress: progress.is_active(),
        };
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
