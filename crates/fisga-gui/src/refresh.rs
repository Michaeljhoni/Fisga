//! "Atualizar endereço" (IDM's "Refresh download address"): a new link to the
//! same file for a download whose link expired. Paste it, or let the Fisga
//! catch it: "Capturar do navegador" opens the page, and the next time the
//! file is downloaded there its link goes to this download.

use gtk::prelude::*;
use gtk::{gio, glib};
use fisga_ipc::{DownloadInfo, Request};

use crate::ipc;
use crate::window::{looks_like_url, show_error};

fn open_page(parent: &gtk::Window, page: &str) {
    let parent_ = parent.clone();
    gtk::UriLauncher::new(page).launch(Some(parent), gio::Cancellable::NONE, move |result| {
        if let Err(e) = result {
            show_error(Some(&parent_), "Não foi possível abrir a página", &e.to_string());
        }
    });
}

pub fn show(parent: &gtk::Window, info: &DownloadInfo) {
    let window = gtk::Window::builder()
        .title("Atualizar endereço")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .resizable(false)
        .build();
    let explanation = gtk::Label::builder()
        .label(format!(
            "O link de \u{201c}{}\u{201d} expirou ou mudou? Cole abaixo um novo endereço do mesmo arquivo. \
             Ou clique em \u{201c}Capturar do navegador\u{201d} e baixe o arquivo de novo na página: o Fisga \
             usa o link novo e continua de onde parou.",
            info.name
        ))
        .wrap(true)
        .xalign(0.0)
        .max_width_chars(60)
        .build();
    let entry = gtk::Entry::builder().placeholder_text("https://…").activates_default(true).hexpand(true).build();
    let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    error.add_css_class("error");

    let page = gtk::Button::builder().label("Abrir a página").visible(info.page.is_some()).build();
    let cancel = gtk::Button::with_label("Cancelar");
    let capture = gtk::Button::with_label(match info.awaiting_url {
        true => "Parar de esperar",
        false => "Capturar do navegador",
    });
    let refresh = gtk::Button::with_label("Atualizar");
    refresh.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).build();
    buttons.append(&page);
    let right = gtk::Box::builder().spacing(8).halign(gtk::Align::End).hexpand(true).build();
    right.append(&cancel);
    right.append(&capture);
    right.append(&refresh);
    buttons.append(&right);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(&explanation);
    content.append(&entry);
    content.append(&error);
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&refresh));

    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    if let Some(url) = info.page.clone() {
        let w = window.clone();
        page.connect_clicked(move |_| open_page(w.upcast_ref(), &url));
    }

    let (w, id, waiting, page_url) = (window.clone(), info.id, !info.awaiting_url, info.page.clone());
    capture.connect_clicked(move |_| {
        let (w, page_url) = (w.clone(), page_url.clone());
        glib::spawn_future_local(async move {
            match ipc::call(Request::AwaitUrl { id, waiting }).await {
                Ok(_) => {
                    // The user downloads the file there again.
                    if let Some(page) = page_url.filter(|_| waiting) {
                        open_page(w.upcast_ref(), &page);
                    }
                    w.close();
                }
                Err(e) => show_error(Some(w.upcast_ref()), "Não foi possível", &e.to_string()),
            }
        });
    });

    let (w, e, err) = (window.clone(), entry.clone(), error.clone());
    refresh.connect_clicked(move |button| {
        let url = e.text().trim().to_string();
        if url.is_empty() {
            return;
        }
        button.set_sensitive(false);
        err.set_visible(false);
        let (w, err, button) = (w.clone(), err.clone(), button.clone());
        glib::spawn_future_local(async move {
            match ipc::call(Request::RefreshUrl { id, url }).await {
                Ok(_) => w.close(),
                Err(e) => {
                    err.set_label(&format!("Não foi possível usar este endereço: {e}"));
                    err.set_visible(true);
                    button.set_sensitive(true);
                }
            }
        });
    });

    // A new link copied from the browser is suggested, as in "Adicionar URL".
    let (e, old) = (entry.clone(), info.url.clone());
    glib::spawn_future_local(async move {
        if let Ok(Some(text)) = e.clipboard().read_text_future().await {
            if e.text().is_empty() && looks_like_url(&text) && text.trim() != old {
                e.set_text(text.trim());
                e.select_region(0, -1);
            }
        }
    });
    window.present();
}
