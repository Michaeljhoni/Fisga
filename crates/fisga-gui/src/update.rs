//! A new version of Fisga: the banner on top of the main window (what it is,
//! its notes, "Atualizar agora" where Fisga installs it itself) and
//! "Procurar atualizações" in the menu. The daemon looks and installs; see
//! `update.rs` there.

use std::cell::RefCell;
use std::rc::Rc;

use fisga_ipc::{NewVersion, Request, Response, UpdateStage};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::ipc;
use crate::window::show_error;

pub struct Banner {
    pub root: gtk::Revealer,
    title: gtk::Label,
    detail: gtk::Label,
    install: gtk::Button,
    /// The release's page, for "Novidades".
    page: Rc<RefCell<String>>,
}

impl Banner {
    /// `dismiss` hides it until another version comes.
    pub fn new(window: &gtk::ApplicationWindow, dismiss: impl Fn() + 'static) -> Self {
        let icon = gtk::Image::from_icon_name("software-update-available-symbolic");
        icon.set_pixel_size(20);
        let title = gtk::Label::builder().xalign(0.0).build();
        title.add_css_class("fisga-update-title");
        let detail = gtk::Label::builder().xalign(0.0).wrap(true).build();
        detail.add_css_class("fisga-update-detail");
        let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
        text.append(&title);
        text.append(&detail);

        let notes = gtk::Button::with_label("Novidades");
        let install = gtk::Button::with_label("Atualizar agora");
        install.add_css_class("suggested-action");
        let close = gtk::Button::builder().icon_name("window-close-symbolic").tooltip_text("Agora não").build();
        close.add_css_class("flat");

        let row = gtk::Box::builder().spacing(12).build();
        row.add_css_class("fisga-update");
        row.append(&icon);
        row.append(&text);
        for button in [&notes, &install, &close] {
            button.set_valign(gtk::Align::Center);
            row.append(button);
        }
        let root = gtk::Revealer::builder().child(&row).transition_type(gtk::RevealerTransitionType::SlideDown).build();

        let page: Rc<RefCell<String>> = Rc::default();
        let (p, w) = (page.clone(), window.clone());
        notes.connect_clicked(move |_| {
            let parent = w.clone();
            gtk::UriLauncher::new(&p.borrow()).launch(Some(&w), gio::Cancellable::NONE, move |result| {
                if let Err(e) = result {
                    show_error(Some(parent.upcast_ref()), "Não foi possível abrir a página", &e.to_string());
                }
            });
        });
        let w = window.clone();
        install.connect_clicked(move |button| {
            // Until the daemon says how it goes.
            button.set_sensitive(false);
            let (w, button) = (w.clone(), button.clone());
            glib::spawn_future_local(async move {
                if let Err(e) = ipc::call(Request::InstallUpdate).await {
                    button.set_sensitive(true);
                    show_error(Some(w.upcast_ref()), "Não foi possível atualizar", &e.to_string());
                }
            });
        });
        close.connect_clicked(move |_| dismiss());
        Self { root, title, detail, install, page }
    }

    /// Shows `release` (`None`: hides the banner).
    pub fn show(&self, release: Option<&NewVersion>) {
        let Some(release) = release else {
            self.root.set_reveal_child(false);
            return;
        };
        self.title.set_label(&format!("Fisga {} disponível", release.version));
        let detail = match (&release.stage, release.installable) {
            (Some(stage), _) => stage.describe(),
            (None, true) => format!(
                "Você está na {}. Os downloads em andamento param durante a atualização e continuam depois.",
                fisga_ipc::VERSION
            ),
            (None, false) => format!("Você está na {}. Atualize pelo gerenciador de pacotes do sistema.", fisga_ipc::VERSION),
        };
        self.detail.set_label(&detail);
        let failed = matches!(release.stage, Some(UpdateStage::Failed { .. }));
        self.install.set_label(if failed { "Tentar de novo" } else { "Atualizar agora" });
        self.install.set_visible(release.installable && release.stage != Some(UpdateStage::Opened));
        self.install.set_sensitive(!release.stage.as_ref().is_some_and(UpdateStage::busy));
        self.page.replace(release.page.clone());
        self.root.set_reveal_child(true);
    }
}

/// "Procurar atualizações": a new version shows in the banner (`found`);
/// none, or a failure, in a dialog.
pub fn check(window: &gtk::ApplicationWindow, found: impl Fn() + 'static) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        match ipc::call(Request::CheckUpdate).await {
            Ok(Response::NewVersion { release: Some(_) }) => found(),
            Ok(Response::NewVersion { release: None }) => gtk::AlertDialog::builder()
                .modal(true)
                .message("O Fisga está atualizado")
                .detail(format!("A versão {} é a mais nova.", fisga_ipc::VERSION))
                .build()
                .show(Some(&window)),
            Ok(_) => {}
            Err(e) => show_error(Some(window.upcast_ref()), "Não foi possível procurar atualizações", &e.to_string()),
        }
    });
}
