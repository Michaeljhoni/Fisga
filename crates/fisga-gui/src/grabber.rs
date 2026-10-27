//! "Baixar de um site" (IDM's Site Grabber): the daemon reads a page (and,
//! up to a depth, the pages below it), this window lists the files found,
//! filtered by type and name, to tick and send to a queue.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, pango};
use fisga_ipc::{Category, GrabbedLink, Queue, Request, Response};

use crate::ipc;
use crate::window::{close_on_escape, looks_like_url};

struct Found {
    link: GrabbedLink,
    check: gtk::CheckButton,
    row: gtk::ListBoxRow,
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(1.0).build();
    label.add_css_class("dim-label");
    label
}

fn found_row(link: &GrabbedLink) -> (gtk::ListBoxRow, gtk::CheckButton) {
    let check = gtk::CheckButton::builder().active(true).build();
    let name = gtk::Label::builder().label(&link.name).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
    let url = gtk::Label::builder().label(&link.url).xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
    url.add_css_class("dim-label");
    url.add_css_class("caption");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    text.append(&name);
    text.append(&url);
    let kind = gtk::Label::new(Some(link.category.label()));
    kind.add_css_class("dim-label");
    let line = gtk::Box::builder().spacing(10).margin_top(4).margin_bottom(4).margin_start(6).margin_end(6).build();
    line.append(&check);
    line.append(&text);
    line.append(&kind);
    let row = gtk::ListBoxRow::builder().child(&line).activatable(false).build();
    (row, check)
}

pub fn show(parent: &gtk::ApplicationWindow, queues: Vec<Queue>) {
    let window = gtk::Window::builder()
        .title("Baixar de um site")
        .transient_for(parent)
        .modal(true)
        .default_width(720)
        .default_height(620)
        .build();

    // Enter = "Procurar" (the default widget).
    let url = gtk::Entry::builder().placeholder_text("https://site.com/pasta/").hexpand(true).activates_default(true).build();
    let depth = gtk::SpinButton::with_range(0.0, fisga_ipc::MAX_GRAB_DEPTH as f64, 1.0);
    depth.set_value(1.0);
    depth.set_tooltip_text(Some("Quantos níveis de páginas seguir: 0 = só esta página"));
    let search = gtk::Button::with_label("Procurar");
    search.add_css_class("suggested-action");
    let spinner = gtk::Spinner::new();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    top.append(&url);
    top.append(&gtk::Label::new(Some("níveis")));
    top.append(&depth);
    top.append(&search);
    top.append(&spinner);
    let hint = gtk::Label::builder()
        .label("Lê a página e, até o número de níveis, as páginas abaixo dela no mesmo site, e lista os arquivos que elas apontam.")
        .xalign(0.0)
        .wrap(true)
        .build();
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");

    // Filters: types (none ticked = all) and name patterns.
    let types = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    types.add_css_class("linked");
    let type_buttons: Vec<(Category, gtk::ToggleButton)> = Category::ALL
        .into_iter()
        .map(|category| {
            let button = gtk::ToggleButton::with_label(category.label());
            types.append(&button);
            (category, button)
        })
        .collect();
    let pattern = gtk::Entry::builder().placeholder_text("*.pdf aula_??.mp4").hexpand(true).build();
    let filters = gtk::Grid::builder().column_spacing(12).row_spacing(8).build();
    filters.attach(&caption("Tipos"), 0, 0, 1, 1);
    filters.attach(&types, 1, 0, 1, 1);
    filters.attach(&caption("Nomes"), 0, 1, 1, 1);
    filters.attach(&pattern, 1, 1, 1, 1);

    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
    list.add_css_class("boxed-list");
    let placeholder = gtk::Label::builder().label("Digite o endereço e clique em Procurar.").margin_top(24).margin_bottom(24).build();
    placeholder.add_css_class("dim-label");
    list.set_placeholder(Some(&placeholder));
    let scroll = gtk::ScrolledWindow::builder().child(&list).vexpand(true).min_content_height(260).build();

    let all = gtk::Button::with_label("Marcar todos");
    let none = gtk::Button::with_label("Desmarcar todos");
    let count = gtk::Label::builder().xalign(1.0).hexpand(true).build();
    count.add_css_class("dim-label");
    let marks = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    marks.append(&all);
    marks.append(&none);
    marks.append(&count);

    // Where they go, as in "Adicionar em lote".
    let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
    let queue = gtk::DropDown::from_strings(&names);
    queue.set_visible(queues.len() > 1);
    let queue_caption = caption("Fila");
    queue_caption.set_visible(queues.len() > 1);
    let dir: Rc<RefCell<Option<PathBuf>>> = Rc::default();
    let dir_label = gtk::Label::builder()
        .label("a de cada tipo de arquivo")
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::Start)
        .build();
    let change_dir = gtk::Button::with_label("Alterar…");
    let dir_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    dir_row.append(&dir_label);
    dir_row.append(&change_dir);
    let start = gtk::CheckButton::builder().label("Começar a baixar (entrar na fila)").active(true).build();
    let options = gtk::Grid::builder().column_spacing(12).row_spacing(8).build();
    options.attach(&queue_caption, 0, 0, 1, 1);
    options.attach(&queue, 1, 0, 1, 1);
    options.attach(&caption("Pasta"), 0, 1, 1, 1);
    options.attach(&dir_row, 1, 1, 1, 1);
    options.attach(&start, 1, 2, 1, 1);

    let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    error.add_css_class("error");
    let cancel = gtk::Button::with_label("Cancelar");
    let add = gtk::Button::with_label("Adicionar");
    add.add_css_class("suggested-action");
    add.set_sensitive(false);
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    buttons.append(&cancel);
    buttons.append(&add);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    for widget in [
        top.upcast_ref::<gtk::Widget>(),
        hint.upcast_ref(),
        filters.upcast_ref(),
        scroll.upcast_ref(),
        marks.upcast_ref(),
        gtk::Separator::new(gtk::Orientation::Horizontal).upcast_ref(),
        options.upcast_ref(),
        error.upcast_ref(),
        buttons.upcast_ref(),
    ] {
        content.append(widget);
    }
    window.set_child(Some(&content));
    window.set_default_widget(Some(&search));
    GtkWindowExt::set_focus(&window, Some(&url));
    close_on_escape(&window);

    let found: Rc<RefCell<Vec<Found>>> = Rc::default();
    // Shows the rows the filters let through, and counts the ticked ones.
    let refresh = {
        let (found, type_buttons, pattern, count, add) = (found.clone(), type_buttons.clone(), pattern.clone(), count.clone(), add.clone());
        Rc::new(move || {
            let wanted: Vec<Category> =
                type_buttons.iter().filter(|(_, button)| button.is_active()).map(|(category, _)| *category).collect();
            let patterns = pattern.text();
            let (mut shown, mut ticked) = (0, 0);
            for item in found.borrow().iter() {
                let visible = fisga_ipc::grab_filter(&item.link, &wanted, &patterns);
                item.row.set_visible(visible);
                shown += visible as usize;
                ticked += (visible && item.check.is_active()) as usize;
            }
            let total = found.borrow().len();
            count.set_label(&match total {
                0 => String::new(),
                _ => format!("{ticked} marcados · {shown} de {total} com os filtros"),
            });
            add.set_label(&match ticked {
                0 => "Adicionar".to_string(),
                n => format!("Adicionar {n}"),
            });
            add.set_sensitive(ticked > 0);
        })
    };
    for (_, button) in &type_buttons {
        let r = refresh.clone();
        button.connect_toggled(move |_| r());
    }
    let r = refresh.clone();
    pattern.connect_changed(move |_| r());
    for (button, active) in [(&all, true), (&none, false)] {
        let (found, r) = (found.clone(), refresh.clone());
        button.connect_clicked(move |_| {
            for item in found.borrow().iter().filter(|item| item.row.is_visible()) {
                item.check.set_active(active);
            }
            r();
        });
    }

    let (u, list_, found_, r, error_, spinner_, w) =
        (url.clone(), list.clone(), found.clone(), refresh.clone(), error.clone(), spinner.clone(), window.clone());
    search.connect_clicked(move |button| {
        let address = u.text().trim().to_string();
        if address.is_empty() {
            return;
        }
        let request = Request::Grab { url: address, depth: depth.value() as u8 };
        button.set_sensitive(false);
        spinner_.start();
        error_.set_visible(false);
        let (button, list, found, r, error, spinner, w) =
            (button.clone(), list_.clone(), found_.clone(), r.clone(), error_.clone(), spinner_.clone(), w.clone());
        glib::spawn_future_local(async move {
            let result = ipc::call(request).await;
            spinner.stop();
            button.set_sensitive(true);
            while let Some(row) = list.row_at_index(0) {
                list.remove(&row);
            }
            found.borrow_mut().clear();
            match result {
                Ok(Response::Grabbed { links, pages }) => {
                    for link in links {
                        let (row, check) = found_row(&link);
                        let r = r.clone();
                        check.connect_toggled(move |_| r());
                        list.append(&row);
                        found.borrow_mut().push(Found { link, check, row });
                    }
                    if found.borrow().is_empty() {
                        error.set_label(&format!("Nenhum arquivo nas {pages} página(s) lida(s)."));
                        error.set_visible(true);
                    }
                    w.set_title(Some(&format!("Baixar de um site — {pages} página(s)")));
                }
                Ok(_) => {}
                Err(e) => {
                    error.set_label(&format!("Não foi possível procurar: {e}"));
                    error.set_visible(true);
                }
            }
            r();
        });
    });

    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    let (w, d, l) = (window.clone(), dir.clone(), dir_label.clone());
    change_dir.connect_clicked(move |_| {
        let (w, d, l) = (w.clone(), d.clone(), l.clone());
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Salvar os arquivos em").modal(true).build();
            if let Ok(folder) = dialog.select_folder_future(Some(&w)).await {
                if let Some(path) = folder.path() {
                    l.set_label(&path.display().to_string());
                    d.replace(Some(path));
                }
            }
        });
    });

    let queue_ids: Vec<u64> = queues.iter().map(|q| q.id).collect();
    let w = window.clone();
    add.connect_clicked(move |button| {
        let urls: Vec<String> = found
            .borrow()
            .iter()
            .filter(|item| item.row.is_visible() && item.check.is_active())
            .map(|item| item.link.url.clone())
            .collect();
        if urls.is_empty() {
            return;
        }
        let request = Request::AddBatch {
            urls,
            output_dir: dir.borrow().clone(),
            queue: queue_ids.get(queue.selected() as usize).copied().filter(|_| queue.is_visible()),
            start: start.is_active(),
        };
        button.set_sensitive(false);
        let (w, button, error) = (w.clone(), button.clone(), error.clone());
        glib::spawn_future_local(async move {
            match ipc::call(request).await {
                Ok(_) => w.close(),
                Err(e) => {
                    error.set_label(&format!("Não foi possível adicionar: {e}"));
                    error.set_visible(true);
                    button.set_sensitive(true);
                }
            }
        });
    });

    // An address in the clipboard is suggested.
    let u = url.clone();
    glib::spawn_future_local(async move {
        if let Ok(Some(text)) = u.clipboard().read_text_future().await {
            if u.text().is_empty() && looks_like_url(&text) {
                u.set_text(text.trim());
                u.select_region(0, -1);
            }
        }
    });
    window.present();
}
