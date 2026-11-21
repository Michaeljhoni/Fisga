//! "Instalar a extensão no Chrome": how to load the extension that ships with
//! Fisga in Chrome, Edge, Brave or Vivaldi (developer mode, "Load unpacked"),
//! with the folder to pick and buttons to copy what has to be typed. The
//! Chrome Web Store does not carry Fisga's extension.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fisga_i18n::tr;
use gtk::prelude::*;
use gtk::{gio, glib, pango};

use crate::window::{close_on_escape, show_error};

/// The unpacked Chrome extension shipped with Fisga: `<prefix>/share/fisga/extension/chrome`
/// next to `<prefix>/bin` (Windows, Linux packages), or in the bundle's
/// `Resources` (macOS, where the program is in `Contents/MacOS`).
pub fn extension_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    extension_dir_near(&exe)
}

fn extension_dir_near(exe: &Path) -> Option<PathBuf> {
    let prefix = exe.parent()?.parent()?;
    [prefix.join("share"), prefix.join("Resources").join("share")]
        .into_iter()
        .map(|share| share.join("fisga").join("extension").join("chrome"))
        .find(|dir| dir.join("manifest.json").is_file())
}

fn text(markup: &str) -> gtk::Label {
    gtk::Label::builder().label(markup).use_markup(true).xalign(0.0).wrap(true).max_width_chars(62).hexpand(true).build()
}

/// A numbered step; `extra` goes under its text.
fn step(number: u32, markup: &str, extra: Option<&gtk::Widget>) -> gtk::Box {
    let badge = gtk::Label::builder().label(number.to_string()).valign(gtk::Align::Start).width_chars(2).build();
    badge.add_css_class("heading");
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    body.append(&text(markup));
    if let Some(extra) = extra {
        body.append(extra);
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&badge);
    row.append(&body);
    row
}

/// Copies `value`, and says so for a moment.
fn copy_button(label: &'static str, value: String) -> gtk::Button {
    let button = gtk::Button::with_label(label);
    button.connect_clicked(move |button| {
        button.clipboard().set_text(&value);
        button.set_label(tr!("Copiado"));
        let weak = button.downgrade();
        glib::timeout_add_local_once(Duration::from_secs(2), move || {
            if let Some(button) = weak.upgrade() {
                button.set_label(label);
            }
        });
    });
    button
}

fn buttons(list: &[&gtk::Button]) -> gtk::Widget {
    let row = gtk::Box::builder().spacing(8).halign(gtk::Align::Start).build();
    for button in list {
        row.append(*button);
    }
    row.upcast()
}

/// A long path broken only after its separators: a break inside a folder name
/// shows a hyphen that is not in it. The zero-width spaces are only shown; the
/// button copies the path itself.
fn breakable(path: &str) -> String {
    path.replace('/', "/\u{200b}").replace('\\', "\\\u{200b}")
}

/// The folder to pick, with buttons to copy it and to open it.
fn folder_widget(dir: PathBuf) -> gtk::Widget {
    let path = dir.display().to_string();
    let label = gtk::Label::builder()
        .label(breakable(&path))
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .max_width_chars(62)
        .build();
    label.add_css_class("monospace");
    let copy = copy_button(tr!("Copiar caminho"), path);
    let open = gtk::Button::with_label(tr!("Abrir a pasta"));
    open.connect_clicked(move |button| {
        let parent = button.root().and_downcast::<gtk::Window>();
        let p = parent.clone();
        gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(parent.as_ref(), gio::Cancellable::NONE, move |result| {
            if let Err(e) = result {
                show_error(p.as_ref(), tr!("Não foi possível abrir a pasta"), &e.to_string());
            }
        });
    });
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&label);
    column.append(&buttons(&[&copy, &open]));
    column.upcast()
}

pub fn show(parent: &gtk::Window) {
    let window = gtk::Window::builder()
        .title(tr!("Instalar a extensão no Chrome"))
        .transient_for(parent)
        .resizable(false)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(22)
        .margin_end(22)
        .build();
    content.append(&text(tr!(
        "A extensão liga o navegador ao Fisga: o botão <b>Baixar</b> sobre os vídeos, a captura dos downloads e o menu do botão direito. No Chrome, Edge, Brave e Vivaldi, ela é instalada a partir da pasta que vem com o Fisga:"
    )));

    let address = copy_button(tr!("Copiar endereço"), "chrome://extensions".to_string());
    content.append(&step(
        1,
        tr!("Abra uma aba nova e vá para <b>chrome://extensions</b> (no Edge, <b>edge://extensions</b>)."),
        Some(&buttons(&[&address])),
    ));
    content.append(&step(
        2,
        tr!("Ligue o <b>Modo do desenvolvedor</b>, no canto de cima, à direita (no Edge, na coluna da esquerda)."),
        None,
    ));
    let folder = match extension_dir() {
        Some(dir) => folder_widget(dir),
        None => text(tr!(
            "Não achei a pasta da extensão junto do Fisga. Ela fica em <b>share/fisga/extension/chrome</b>, na pasta onde o Fisga foi instalado."
        ))
        .upcast(),
    };
    content.append(&step(
        3,
        tr!("Clique em <b>Carregar sem compactação</b> e escolha esta pasta (na janela que abre, dá para colar o caminho na barra de endereço):"),
        Some(&folder),
    ));
    content.append(&step(
        4,
        tr!("Pronto: a <b>Integração Fisga</b> aparece na lista. Fixe o ícone dela (na peça de quebra-cabeça, ao lado da barra de endereço): o popup mostra se ela está conectada ao Fisga."),
        None,
    ));

    let note = text(tr!(
        "Deixe o modo do desenvolvedor ligado e a pasta onde está: sem eles, o navegador desliga a extensão. As atualizações do Fisga trazem a extensão nova junto, e o navegador passa a usá-la quando é aberto de novo."
    ));
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    content.append(&note);

    let close = gtk::Button::builder().label(tr!("Fechar")).halign(gtk::Align::End).build();
    let w = window.downgrade();
    close.connect_clicked(move |_| w.upgrade().iter().for_each(|window| window.close()));
    content.append(&close);

    window.set_child(Some(&content));
    close_on_escape(&window);
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path, share: &str) -> PathBuf {
        let dir = root.join(share).join("fisga/extension/chrome");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), "{}").unwrap();
        dir
    }

    #[test]
    fn finds_the_extension_next_to_bin_and_in_the_bundle() {
        let root = std::env::temp_dir().join(format!("fisga-chrome-guide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // Windows and Linux packages: <prefix>/bin and <prefix>/share.
        let prefix = root.join("prefix");
        let dir = layout(&prefix, "share");
        assert_eq!(extension_dir_near(&prefix.join("bin/fisga-gui")), Some(dir));

        // macOS: Contents/MacOS and Contents/Resources/share.
        let contents = root.join("Fisga.app/Contents");
        let dir = layout(&contents, "Resources/share");
        assert_eq!(extension_dir_near(&contents.join("MacOS/fisga-gui")), Some(dir));

        // A build without it (cargo run).
        assert_eq!(extension_dir_near(&root.join("target/debug/fisga-gui")), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_paths_break_after_separators() {
        assert_eq!(breakable("/usr/share/fisga").replace('\u{200b}', ""), "/usr/share/fisga");
        assert_eq!(breakable(r"C:\Fisga\share").matches('\u{200b}').count(), 2);
    }
}
