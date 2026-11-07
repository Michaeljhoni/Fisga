//! "Propriedades" (IDM's download properties): everything known about a
//! download, to read and copy. It follows the list row while open.

use std::cell::RefCell;
use std::rc::Rc;

use fisga_i18n::{tr, trf};
use fisga_ipc::{DownloadInfo, Hashing, Queue, SourceKind, State};
use gtk::prelude::*;
use gtk::{glib, pango};

use crate::format;
use crate::item::DownloadItem;

/// The lines of the window, in order: a caption and its text (empty = hidden).
fn lines(info: &DownloadInfo, queues: &[Queue]) -> Vec<(&'static str, String)> {
    let kind = match info.kind {
        SourceKind::Direct => tr!("Link direto"),
        SourceKind::Page => tr!("Vídeo de site (pelo yt-dlp)"),
        SourceKind::Stream => tr!("Transmissão (HLS/DASH)"),
    };
    let file = match (&info.path, &info.dir) {
        (Some(path), _) => path.display().to_string(),
        (None, Some(dir)) => dir.join(&info.name).display().to_string(),
        (None, None) => String::new(),
    };
    let downloaded = match (info.state, info.total) {
        (State::Completed, _) => String::new(),
        (_, Some(total)) if total > 0 => {
            trf!("{} de {} ({})", format::bytes(info.downloaded), format::bytes(total), format::progress_text(info))
        }
        _ if info.downloaded > 0 => format::bytes(info.downloaded),
        _ => String::new(),
    };
    let connections = match (info.kind, info.state) {
        (SourceKind::Direct, State::Running) => info.connections.to_string(),
        _ => String::new(),
    };
    let limit = match info.speed_limit {
        0 => String::new(),
        limit => format!("{}/s", format::bytes(limit)),
    };
    let expected = info.checksum.as_ref().map(|c| format!("{}: {}", c.algo.label(), c.hex)).unwrap_or_default();
    let found = match &info.hash {
        Some(Hashing::Done { digest }) => format!("{}: {}", digest.algo.label(), digest.hex),
        _ => String::new(),
    };
    let queue = queues.iter().find(|queue| queue.id == info.queue).map(|queue| queue.name.clone()).unwrap_or_default();
    vec![
        (tr!("Nome"), info.name.clone()),
        (tr!("Situação"), format::status(info)),
        (tr!("Endereço"), info.url.clone()),
        (tr!("Página"), info.page.clone().filter(|page| *page != info.url).unwrap_or_default()),
        (tr!("Arquivo"), file),
        (tr!("Tamanho"), format::size(info)),
        (tr!("Baixado"), downloaded),
        (tr!("Tipo"), kind.to_string()),
        (tr!("Categoria"), info.category.label().to_string()),
        (tr!("Fila"), queue),
        (tr!("Adicionado em"), format::added(info)),
        (tr!("Conexões"), connections),
        (tr!("Limite próprio"), limit),
        (tr!("Soma esperada"), expected),
        (tr!("Soma do arquivo"), found),
    ]
}

pub fn show(parent: &gtk::Window, item: &DownloadItem, queues: Vec<Queue>) {
    let info = item.info();
    let window = gtk::Window::builder()
        .title(tr!("Propriedades"))
        .transient_for(parent)
        .default_width(620)
        .resizable(false)
        .build();

    let grid = gtk::Grid::builder().column_spacing(14).row_spacing(8).build();
    let mut values = Vec::new();
    for (row, (caption, _)) in lines(&info, &queues).into_iter().enumerate() {
        let caption = gtk::Label::builder().label(caption).xalign(1.0).yalign(0.0).build();
        caption.add_css_class("dim-label");
        let value = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(pango::WrapMode::WordChar)
            .max_width_chars(64)
            .selectable(true)
            .hexpand(true)
            .build();
        grid.attach(&caption, 0, row as i32, 1, 1);
        grid.attach(&value, 1, row as i32, 1, 1);
        values.push((caption, value));
    }
    let values = Rc::new(values);
    let fill = {
        let values = values.clone();
        move |info: &DownloadInfo| {
            for ((caption, value), (_, text)) in values.iter().zip(lines(info, &queues)) {
                caption.set_visible(!text.is_empty());
                value.set_visible(!text.is_empty());
                value.set_label(&text);
            }
        }
    };
    fill(&info);

    let copy = gtk::Button::with_label(tr!("Copiar endereço"));
    let close = gtk::Button::with_label(tr!("Fechar"));
    close.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    buttons.append(&copy);
    buttons.append(&close);
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
    window.set_default_widget(Some(&close));
    crate::window::close_on_escape(&window);
    // Not on a selectable label: it would start with its text selected.
    GtkWindowExt::set_focus(&window, Some(&close));

    // The texts follow the download while the window is open.
    let mut follows = Vec::new();
    for property in ["status", "size", "progress-text"] {
        let fill = fill.clone();
        follows.push(item.connect_notify_local(Some(property), move |item, _| item.with_info(&fill)));
    }
    let follows = RefCell::new(follows);
    let item_ = item.clone();
    window.connect_close_request(move |_| {
        for handler in follows.take() {
            item_.disconnect(handler);
        }
        glib::Propagation::Proceed
    });

    let (w, item_) = (window.clone(), item.clone());
    copy.connect_clicked(move |_| w.clipboard().set_text(&item_.with_info(|info| info.url.clone())));
    let w = window.clone();
    close.connect_clicked(move |_| w.close());
    window.present();
}
