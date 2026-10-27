//! "Conferir soma": the SHA-256 (or SHA-1/384/512) of a finished file,
//! compared with the one its download page published. The window follows
//! the list row, so the check's progress and result show as they arrive.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use fisga_ipc::{Checksum, DownloadInfo, Hashing, Request, State};

use crate::ipc;
use crate::item::DownloadItem;

struct Widgets {
    entry: gtk::Entry,
    progress: gtk::ProgressBar,
    digest: gtk::Label,
    verdict: gtk::Label,
    error: gtk::Label,
    check: gtk::Button,
}

impl Widgets {
    fn show(&self, info: &DownloadInfo) {
        let finished = info.state == State::Completed;
        let running = matches!(info.hash, Some(Hashing::Running { .. }));
        self.check.set_label(if finished { "Conferir" } else { "Guardar" });
        self.check.set_sensitive(!running);
        self.entry.set_sensitive(!running);
        self.progress.set_visible(running);
        self.digest.set_visible(false);
        self.verdict.set_visible(false);
        self.verdict.remove_css_class("success");
        self.verdict.remove_css_class("error");
        match &info.hash {
            Some(Hashing::Running { done }) => {
                let total = info.total.unwrap_or(info.downloaded).max(1);
                self.progress.set_fraction((*done as f64 / total as f64).min(1.0));
                self.progress.set_text(Some(&format!("Lendo o arquivo… {}", crate::format::bytes(*done))));
            }
            Some(Hashing::Done { digest }) => {
                self.digest.set_label(&format!("{} do arquivo:\n{}", digest.algo.label(), digest.hex));
                self.digest.set_visible(true);
                let verdict = match info.checksum_matches() {
                    Some(true) => Some(("success", "\u{2713} Confere: é exatamente o arquivo publicado na página.")),
                    Some(false) => Some((
                        "error",
                        "\u{2717} Não confere: o arquivo pode estar corrompido ou ter sido trocado. Baixe-o de novo \
                         e, se continuar diferente, desconfie do site.",
                    )),
                    None => None,
                };
                if let Some((class, text)) = verdict {
                    self.verdict.set_label(text);
                    self.verdict.add_css_class(class);
                    self.verdict.set_visible(true);
                }
            }
            Some(Hashing::Failed { error }) => {
                self.verdict.set_label(&format!("Não foi possível conferir: {error}"));
                self.verdict.add_css_class("error");
                self.verdict.set_visible(true);
            }
            None => {}
        }
    }
}

pub fn show(parent: &gtk::Window, item: &DownloadItem) {
    let info = item.info();
    let window = gtk::Window::builder()
        .title("Conferir soma")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .resizable(false)
        .build();
    let explanation = gtk::Label::builder()
        .label(match info.state {
            State::Completed => format!(
                "Cole a soma publicada na página de download (SHA-256, SHA-1, SHA-384 ou SHA-512) para conferir se \
                 \u{201c}{}\u{201d} é exatamente aquele arquivo. Sem ela, o Fisga só mostra a SHA-256 do arquivo.",
                info.name
            ),
            _ => format!(
                "Cole a soma publicada na página de download (SHA-256, SHA-1, SHA-384 ou SHA-512): quando \
                 \u{201c}{}\u{201d} terminar, o Fisga confere se é exatamente aquele arquivo.",
                info.name
            ),
        })
        .wrap(true)
        .xalign(0.0)
        .max_width_chars(60)
        .build();
    let entry = gtk::Entry::builder()
        .placeholder_text("soma da página (opcional)")
        .text(info.checksum.as_ref().map(|c| c.hex.as_str()).unwrap_or_default())
        .activates_default(true)
        .hexpand(true)
        .build();
    let progress = gtk::ProgressBar::builder().show_text(true).visible(false).build();
    let digest = gtk::Label::builder().xalign(0.0).wrap(true).wrap_mode(gtk::pango::WrapMode::Char).selectable(true).build();
    digest.add_css_class("monospace");
    let verdict = gtk::Label::builder().xalign(0.0).wrap(true).build();
    let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    error.add_css_class("error");

    let close = gtk::Button::with_label("Fechar");
    let check = gtk::Button::with_label("Conferir");
    check.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    buttons.append(&close);
    buttons.append(&check);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    for widget in [
        explanation.upcast_ref::<gtk::Widget>(),
        entry.upcast_ref(),
        progress.upcast_ref(),
        digest.upcast_ref(),
        verdict.upcast_ref(),
        error.upcast_ref(),
        buttons.upcast_ref(),
    ] {
        content.append(widget);
    }
    window.set_child(Some(&content));
    window.set_default_widget(Some(&check));
    crate::window::close_on_escape(&window);

    let widgets = Rc::new(Widgets { entry, progress, digest, verdict, error, check });
    widgets.show(&info);
    let w = widgets.clone();
    let follow = item.connect_notify_local(Some("status"), move |item, _| w.show(&item.info()));
    let follow = RefCell::new(Some(follow));
    let item_ = item.clone();
    window.connect_close_request(move |_| {
        if let Some(handler) = follow.take() {
            item_.disconnect(handler);
        }
        glib::Propagation::Proceed
    });

    let win = window.clone();
    close.connect_clicked(move |_| win.close());
    let (w, win, id, finished) = (widgets.clone(), window.clone(), info.id, info.state == State::Completed);
    widgets.check.connect_clicked(move |_| {
        let checksum = match w.entry.text().trim() {
            "" if !finished => return,
            "" => None,
            text => match Checksum::parse(text) {
                Ok(checksum) => Some(checksum),
                Err(e) => {
                    w.error.set_label(&format!("Soma inválida: {e}"));
                    w.error.set_visible(true);
                    return;
                }
            },
        };
        w.error.set_visible(false);
        w.check.set_sensitive(false);
        let (w, win) = (w.clone(), win.clone());
        glib::spawn_future_local(async move {
            match ipc::call(Request::Verify { id, checksum }).await {
                // A finished one shows the check here as it runs.
                Ok(_) if !finished => win.close(),
                Ok(_) => {}
                Err(e) => {
                    w.error.set_label(&format!("Não foi possível conferir: {e}"));
                    w.error.set_visible(true);
                    w.check.set_sensitive(true);
                }
            }
        });
    });

    // A checksum copied from the download page is suggested.
    let e = widgets.entry.clone();
    glib::spawn_future_local(async move {
        if let Ok(Some(text)) = e.clipboard().read_text_future().await {
            if e.text().is_empty() && text.len() < 1024 {
                if let Ok(checksum) = Checksum::parse(&text) {
                    e.set_text(&checksum.hex);
                    e.select_region(0, -1);
                }
            }
        }
    });
    window.present();
}
