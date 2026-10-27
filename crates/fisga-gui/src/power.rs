//! The countdown before the computer is suspended or turned off at the end of
//! a scheduled queue. "Cancelar", Esc or closing the window calls it off.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gtk::prelude::*;
use gtk::glib;
use fisga_ipc::{PowerPlan, Request, WhenDone};

use crate::window::send;

pub struct PowerDialog {
    pub plan: PowerPlan,
    window: gtk::Window,
    /// Closed because the daemon called it off (or it happened): no request.
    external: Rc<Cell<bool>>,
}

fn remaining(plan: &PowerPlan) -> u64 {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    plan.at.saturating_sub(now)
}

pub fn text(plan: &PowerPlan) -> String {
    let what = match plan.action {
        WhenDone::Suspend => "suspenso",
        _ => "desligado",
    };
    format!("Os downloads agendados terminaram.\nO computador será {what} em {} s.", remaining(plan))
}

impl PowerDialog {
    pub fn new(app: &gtk::Application, parent: Option<&gtk::ApplicationWindow>, plan: PowerPlan) -> Self {
        let window = gtk::Window::builder()
            .application(app)
            .title("Fila concluída")
            .modal(parent.is_some())
            .resizable(false)
            .default_width(380)
            .build();
        window.set_transient_for(parent);
        let label = gtk::Label::builder().label(text(&plan)).xalign(0.0).wrap(true).build();
        let cancel = gtk::Button::with_label("Cancelar");
        cancel.add_css_class("suggested-action");
        let buttons = gtk::Box::builder().halign(gtk::Align::End).build();
        buttons.append(&cancel);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        content.append(&label);
        content.append(&buttons);
        window.set_child(Some(&content));
        window.set_default_widget(Some(&cancel));

        let external = Rc::new(Cell::new(false));
        let w = window.clone();
        cancel.connect_clicked(move |_| w.close());
        let ext = external.clone();
        window.connect_close_request(move |_| {
            if !ext.get() {
                send(Request::CancelPower, None);
            }
            glib::Propagation::Proceed
        });
        let key = gtk::EventControllerKey::new();
        let w = window.clone();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                w.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key);

        // Stops when the window is gone.
        let (l, p) = (label.downgrade(), plan.clone());
        glib::timeout_add_local(Duration::from_millis(500), move || match l.upgrade() {
            Some(label) => {
                label.set_label(&text(&p));
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        Self { plan, window, external }
    }

    /// False once the user closed it (the daemon may still announce the plan,
    /// e.g. when cancelling failed).
    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn close_externally(&self) {
        self.external.set(true);
        self.window.close();
    }
}
