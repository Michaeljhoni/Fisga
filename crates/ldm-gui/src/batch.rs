//! "Adicionar em lote" (IDM's batch download): many addresses at once,
//! straight to a queue. Either a pattern with `*` and a range (aula_01 to
//! aula_20, parte-a to parte-f) or a list of links (pasted, or taken from the
//! clipboard), with a preview of what will be added.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, pango};
use ldm_ipc::{Queue, Request};

use crate::ipc;

/// What the pattern tab or the list tab gives.
fn addresses(
    pattern: bool,
    template: &gtk::Entry,
    from: &gtk::Entry,
    to: &gtk::Entry,
    width: &gtk::SpinButton,
    list: &gtk::TextBuffer,
) -> Result<Vec<String>, String> {
    if pattern {
        return ldm_ipc::expand_batch(&template.text(), &from.text(), &to.text(), width.value() as usize);
    }
    let text = list.text(&list.start_iter(), &list.end_iter(), false);
    let urls = ldm_ipc::parse_url_list(&text);
    match urls.len() {
        0 => Err("Cole os endereços acima, um por linha.".into()),
        n if n > ldm_ipc::MAX_BATCH => Err(format!("São {n} endereços; o máximo é {}.", ldm_ipc::MAX_BATCH)),
        _ => Ok(urls),
    }
}

fn preview(urls: &[String]) -> String {
    match urls {
        [] => String::new(),
        [one] => format!("1 endereço: {one}"),
        [first, .., last] => format!("{} endereços: {first} … {last}", urls.len()),
    }
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(text).xalign(1.0).build();
    label.add_css_class("dim-label");
    label
}

pub fn show(parent: &gtk::ApplicationWindow, queues: Vec<Queue>) {
    let window = gtk::Window::builder()
        .title("Adicionar em lote")
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .resizable(false)
        .build();

    // Pattern tab.
    let template = gtk::Entry::builder().placeholder_text("https://site.com/aula_*.mp4").hexpand(true).build();
    let from = gtk::Entry::builder().text("1").width_chars(6).build();
    let to = gtk::Entry::builder().text("10").width_chars(6).build();
    let width = gtk::SpinButton::with_range(0.0, 10.0, 1.0);
    let range = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    range.append(&from);
    range.append(&gtk::Label::new(Some("até")));
    range.append(&to);
    range.append(&gtk::Label::new(Some("  dígitos")));
    range.append(&width);
    let hint = gtk::Label::builder()
        .label("O * vira cada número (ou letra, de a até z). Começar em 01 deixa todos com 2 dígitos; ou escolha os dígitos (0 = como no início).")
        .xalign(0.0)
        .wrap(true)
        .max_width_chars(70)
        .build();
    hint.add_css_class("dim-label");
    hint.add_css_class("caption");
    let pattern_grid = gtk::Grid::builder().column_spacing(12).row_spacing(10).margin_top(12).margin_start(6).margin_end(6).build();
    pattern_grid.attach(&caption("Endereço"), 0, 0, 1, 1);
    pattern_grid.attach(&template, 1, 0, 1, 1);
    pattern_grid.attach(&caption("De"), 0, 1, 1, 1);
    pattern_grid.attach(&range, 1, 1, 1, 1);
    pattern_grid.attach(&hint, 1, 2, 1, 1);

    // List tab.
    let list = gtk::TextView::builder().wrap_mode(gtk::WrapMode::WordChar).monospace(true).build();
    let list_scroll = gtk::ScrolledWindow::builder().child(&list).min_content_height(150).margin_top(12).build();
    let buffer = list.buffer();

    let notebook = gtk::Notebook::new();
    notebook.append_page(&pattern_grid, Some(&gtk::Label::new(Some("Padrão com *"))));
    notebook.append_page(&list_scroll, Some(&gtk::Label::new(Some("Lista de endereços"))));

    let summary = gtk::Label::builder().xalign(0.0).ellipsize(pango::EllipsizeMode::Middle).build();
    let error = gtk::Label::builder().xalign(0.0).wrap(true).visible(false).build();
    error.add_css_class("error");

    // Where they go.
    let names: Vec<&str> = queues.iter().map(|q| q.name.as_str()).collect();
    let queue = gtk::DropDown::from_strings(&names);
    queue.set_halign(gtk::Align::Start);
    let queue_caption = caption("Fila");
    queue.set_visible(queues.len() > 1);
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
    let options = gtk::Grid::builder().column_spacing(12).row_spacing(10).build();
    options.attach(&queue_caption, 0, 0, 1, 1);
    options.attach(&queue, 1, 0, 1, 1);
    options.attach(&caption("Pasta"), 0, 1, 1, 1);
    options.attach(&dir_row, 1, 1, 1, 1);
    options.attach(&start, 1, 2, 1, 1);

    let cancel = gtk::Button::with_label("Cancelar");
    let add = gtk::Button::with_label("Adicionar");
    add.add_css_class("suggested-action");
    let buttons = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    buttons.append(&cancel);
    buttons.append(&add);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    content.append(&notebook);
    content.append(&summary);
    content.append(&error);
    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    content.append(&options);
    content.append(&buttons);
    window.set_child(Some(&content));
    window.set_default_widget(Some(&add));
    GtkWindowExt::set_focus(&window, Some(&template));

    // The preview follows every change.
    let refresh = {
        let (notebook, template, from, to, width, buffer, summary, add) =
            (notebook.clone(), template.clone(), from.clone(), to.clone(), width.clone(), buffer.clone(), summary.clone(), add.clone());
        Rc::new(move || {
            let result = addresses(notebook.current_page() == Some(0), &template, &from, &to, &width, &buffer);
            match result {
                Ok(urls) => {
                    summary.set_label(&preview(&urls));
                    summary.remove_css_class("dim-label");
                    add.set_sensitive(true);
                }
                Err(message) => {
                    let mut chars = message.chars();
                    let message: String = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
                    summary.set_label(&message);
                    summary.add_css_class("dim-label");
                    add.set_sensitive(false);
                }
            }
        })
    };
    for entry in [&template, &from, &to] {
        let r = refresh.clone();
        entry.connect_changed(move |_| r());
    }
    let r = refresh.clone();
    width.connect_value_changed(move |_| r());
    let r = refresh.clone();
    buffer.connect_changed(move |_| r());
    // `switch-page` runs before the page changes: look again right after.
    let r = refresh.clone();
    notebook.connect_switch_page(move |_, _, _| {
        let r = r.clone();
        glib::idle_add_local_once(move || r());
    });
    refresh();

    // Links in the clipboard: the list tab, filled.
    let (b, n) = (buffer.clone(), notebook.clone());
    glib::spawn_future_local(async move {
        if let Ok(Some(text)) = n.clipboard().read_text_future().await {
            let urls = ldm_ipc::parse_url_list(&text);
            if urls.len() > 1 && b.char_count() == 0 {
                b.set_text(&(urls.join("\n") + "\n"));
                n.set_current_page(Some(1));
            }
        }
    });

    let w = window.clone();
    cancel.connect_clicked(move |_| w.close());
    crate::window::close_on_escape(&window);
    let (w, d, l) = (window.clone(), dir.clone(), dir_label.clone());
    change_dir.connect_clicked(move |_| {
        let (w, d, l) = (w.clone(), d.clone(), l.clone());
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().title("Salvar o lote em").modal(true).build();
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
        let Ok(urls) = addresses(notebook.current_page() == Some(0), &template, &from, &to, &width, &buffer) else {
            return;
        };
        let request = Request::AddBatch {
            urls,
            output_dir: dir.borrow().clone(),
            queue: queue_ids.get(queue.selected() as usize).copied().filter(|_| queue.is_visible()),
            start: start.is_active(),
        };
        button.set_sensitive(false);
        error.set_visible(false);
        let (w, button, error) = (w.clone(), button.clone(), error.clone());
        glib::spawn_future_local(async move {
            match ipc::call(request).await {
                Ok(_) => w.close(),
                Err(e) => {
                    error.set_label(&format!("Não foi possível adicionar o lote: {e}"));
                    error.set_visible(true);
                    button.set_sensitive(true);
                }
            }
        });
    });
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_text() {
        assert_eq!(preview(&[]), "");
        assert_eq!(preview(&["https://e.com/a".to_string()]), "1 endereço: https://e.com/a");
        let urls: Vec<String> = (1..=3).map(|n| format!("https://e.com/{n}")).collect();
        assert_eq!(preview(&urls), "3 endereços: https://e.com/1 … https://e.com/3");
    }
}
