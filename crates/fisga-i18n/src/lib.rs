//! Fisga's texts are written in Portuguese, in the code, and shown in the
//! language of the system (or the one chosen in Preferências): Portuguese
//! or English.
//!
//! Each crate keeps the English of its texts in `i18n/en.txt`, each under its
//! Portuguese, and declares the table at its root:
//!
//! ```ignore
//! static I18N: fisga_i18n::Table = fisga_i18n::Table::new(include_str!("../i18n/en.txt"));
//! ```
//!
//! Then `tr!("Baixar agora")` is "Download now" in English, and
//! `trf!("{} de {} MB", done, total)` fills the `{}` in order (the English
//! may reorder them with `{0}`, `{1}`...). A test in each crate
//! ([`check_crate`]) fails when a text has no English or an English has no
//! text left.
//!
//! The file: blocks separated by blank lines, the Portuguese on the first
//! line, the English on the second, after `= `. Lines starting with `#` are
//! comments. `\n` is a line break in both.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Portuguese,
    English,
}

/// Until set, the system's.
static CURRENT: AtomicU8 = AtomicU8::new(UNSET);
const UNSET: u8 = 0;
const PORTUGUESE: u8 = 1;
const ENGLISH: u8 = 2;

/// The language the texts are shown in from now on.
pub fn set(lang: Lang) {
    CURRENT.store(if lang == Lang::English { ENGLISH } else { PORTUGUESE }, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        PORTUGUESE => Lang::Portuguese,
        ENGLISH => Lang::English,
        _ => {
            let lang = system();
            set(lang);
            lang
        }
    }
}

/// Portuguese when the system is in Portuguese, else English.
/// `$FISGA_LANG` (`pt`, `en`) overrides the system.
pub fn system() -> Lang {
    if let Some(lang) = std::env::var("FISGA_LANG").ok().and_then(|tag| from_tag(&tag)) {
        return lang;
    }
    system_language().unwrap_or(Lang::English)
}

/// `pt_BR.UTF-8`, `pt-PT`, `en_US` → the language; `C`, `POSIX`, empty → none.
pub fn from_tag(tag: &str) -> Option<Lang> {
    let tag = tag.trim().trim_matches('"').to_ascii_lowercase();
    match tag.get(..2) {
        _ if tag.is_empty() || tag == "c" || tag.starts_with("c.") || tag == "posix" => None,
        Some("pt") => Some(Lang::Portuguese),
        Some(_) => Some(Lang::English),
        None => None,
    }
}

/// Windows: the language of its interface (Git Bash and others set `LANG`
/// to English whatever Windows is in).
#[cfg(windows)]
fn system_language() -> Option<Lang> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    const LANG_PORTUGUESE: u16 = 0x16;
    // SAFETY: no arguments, always succeeds.
    let language = unsafe { GetUserDefaultUILanguage() };
    Some(if language & 0x3ff == LANG_PORTUGUESE { Lang::Portuguese } else { Lang::English })
}

/// The locale's variables; an app opened from Finder has none, and then the
/// first of the user's languages in the system's settings.
#[cfg(target_os = "macos")]
fn system_language() -> Option<Lang> {
    locale_variables().or_else(|| {
        let output = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let first = text.split(['(', ',', ')']).map(str::trim).find(|entry| !entry.is_empty())?;
        from_tag(first)
    })
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_language() -> Option<Lang> {
    locale_variables()
}

/// The first of `LC_ALL`, `LC_MESSAGES` and `LANG` that says a language.
#[cfg(not(windows))]
fn locale_variables() -> Option<Lang> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|name| std::env::var(name).ok().and_then(|tag| from_tag(&tag)))
}

/// A crate's English texts, read the first time one is needed.
pub struct Table {
    text: &'static str,
    english: OnceLock<HashMap<String, String>>,
}

impl Table {
    pub const fn new(text: &'static str) -> Table {
        Table { text, english: OnceLock::new() }
    }

    fn english(&self) -> &HashMap<String, String> {
        self.english.get_or_init(|| parse(self.text).into_iter().collect())
    }

    /// `portuguese` in the current language. A text without English stays
    /// in Portuguese (the tests catch it first).
    pub fn translate(&'static self, portuguese: &'static str) -> &'static str {
        match current() {
            Lang::Portuguese => portuguese,
            Lang::English => self.english().get(portuguese).map_or(portuguese, String::as_str),
        }
    }
}

fn unescape(text: &str) -> String {
    text.replace("\\n", "\n")
}

/// The (Portuguese, English) pairs of a table's text.
pub fn parse(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut portuguese: Option<&str> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') {
            continue;
        }
        match (portuguese, line.strip_prefix("= ")) {
            (Some(pt), Some(en)) => {
                pairs.push((unescape(pt), unescape(en)));
                portuguese = None;
            }
            _ if line.trim().is_empty() => portuguese = None,
            _ => portuguese = Some(line),
        }
    }
    pairs
}

/// Fills a template's `{}` in order, and `{0}`, `{1}`... by position.
/// `{{` and `}}` are braces.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut index = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    index.push(c);
                }
                let at = match index.parse::<usize>() {
                    Ok(at) => at,
                    Err(_) => {
                        next += 1;
                        next - 1
                    }
                };
                if let Some(arg) = args.get(at) {
                    out.push_str(&arg.to_string());
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// A text in the current language.
#[macro_export]
macro_rules! tr {
    ($portuguese:literal) => {
        crate::I18N.translate($portuguese)
    };
}

/// A text in the current language, with its `{}` filled.
#[macro_export]
macro_rules! trf {
    ($portuguese:literal $(, $arg:expr)* $(,)?) => {
        $crate::fill(crate::I18N.translate($portuguese), &[$(&$arg as &dyn ::std::fmt::Display),*])
    };
}

/// The value of a Rust string literal starting at `source` (after its
/// opening quote), and the rest after its closing quote.
fn literal(source: &str) -> Option<(String, &str)> {
    let mut value = String::new();
    let mut chars = source.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => return Some((value, &source[at + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => value.push('\n'),
                't' => value.push('\t'),
                '\\' => value.push('\\'),
                '"' => value.push('"'),
                '\'' => value.push('\''),
                '0' => value.push('\0'),
                'u' => {
                    let rest = &source[chars.next()?.0 + 1..];
                    let end = rest.find('}')?;
                    value.push(char::from_u32(u32::from_str_radix(&rest[..end], 16).ok()?)?);
                    for _ in 0..=end {
                        chars.next();
                    }
                }
                // A line continuation: the line break and the next line's indentation go.
                '\n' | '\r' => {
                    let mut ahead = chars.clone();
                    while let Some((_, c)) = ahead.next() {
                        if !c.is_whitespace() {
                            break;
                        }
                        chars.next();
                    }
                }
                other => {
                    value.push('\\');
                    value.push(other);
                }
            },
            c => value.push(c),
        }
    }
    None
}

/// The texts given to `tr!` and `trf!` in Rust code.
pub fn texts_in(source: &str) -> Vec<String> {
    let mut texts = Vec::new();
    let mut rest = source;
    loop {
        // The first `tr!(` or `trf!(` that is a name of its own (not `include_str!(`).
        let found = ["tr!(", "trf!("]
            .iter()
            .flat_map(|name| rest.match_indices(name).map(move |(at, _)| (at, name.len())))
            .filter(|(at, _)| {
                let before = rest[..*at].chars().next_back();
                !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
            .min();
        let Some((at, len)) = found else { break };
        let after = rest[at + len..].trim_start();
        match after.strip_prefix('"').and_then(literal) {
            Some((text, next)) => {
                texts.push(text);
                rest = next;
            }
            None => rest = &rest[at + len..],
        }
    }
    texts
}

/// The placeholders of a template: how many `{}` and which `{N}`.
fn placeholders(template: &str) -> usize {
    let template = template.replace("{{", "").replace("}}", "");
    let mut count = 0;
    let mut highest = 0;
    let mut rest = template.as_str();
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        match rest[open + 1..open + close].parse::<usize>() {
            Ok(index) => highest = highest.max(index + 1),
            Err(_) => count += 1,
        }
        rest = &rest[open + close + 1..];
    }
    count.max(highest)
}

/// What is wrong with a crate's table: texts in its code (`src/`) without
/// English, English for texts no longer there, and English with another
/// number of `{}` than its text. Empty = all good.
pub fn check_crate(manifest_dir: &str, table_text: &str) -> Vec<String> {
    fn sources(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut code = Vec::new();
    sources(&Path::new(manifest_dir).join("src"), &mut code);
    let used: Vec<String> = code.iter().flat_map(|source| texts_in(source)).collect();
    let pairs = parse(table_text);
    let english: HashMap<&str, &str> = pairs.iter().map(|(pt, en)| (pt.as_str(), en.as_str())).collect();
    let mut problems = Vec::new();
    for text in &used {
        match english.get(text.as_str()) {
            None => problems.push(format!("sem inglês: {text:?}")),
            Some(en) if placeholders(en) != placeholders(text) => {
                problems.push(format!("o inglês tem outros {{}}: {text:?} = {en:?}"))
            }
            Some(_) => {}
        }
    }
    for (pt, _) in &pairs {
        if !used.contains(pt) {
            problems.push(format!("inglês de um texto que não existe mais: {pt:?}"));
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

/// The tests' table (`tr!` reads the crate's `I18N`).
#[cfg(test)]
static I18N: Table = Table::new("# teste\nBaixar agora\n= Download now\n\n{} de {}\n= {1} of {0}\n\nDuas\\nlinhas\n= Two\\nlines\n");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        let pairs = parse(I18N.text);
        assert_eq!(pairs[0], ("Baixar agora".to_string(), "Download now".to_string()));
        assert_eq!(pairs[2].1, "Two\nlines");
        assert_eq!(pairs.len(), 3);
    }

    #[test]
    fn filling() {
        assert_eq!(fill("{} de {} MB", &[&3, &10]), "3 de 10 MB");
        assert_eq!(fill("{1} of {0}", &[&"a", &"b"]), "b of a");
        assert_eq!(fill("{{literal}} {}", &[&1]), "{literal} 1");
    }

    #[test]
    fn languages() {
        assert_eq!(from_tag("pt_BR.UTF-8"), Some(Lang::Portuguese));
        assert_eq!(from_tag("en_US"), Some(Lang::English));
        assert_eq!(from_tag("de"), Some(Lang::English));
        assert_eq!(from_tag("C.UTF-8"), None);
        assert_eq!(from_tag(""), None);
        // One test sets the language, so the order of the tests does not matter.
        set(Lang::English);
        assert_eq!(tr!("Baixar agora"), "Download now");
        assert_eq!(trf!("{} de {}", 3, 10), "10 of 3");
        assert_eq!(tr!("Sem inglês"), "Sem inglês");
        set(Lang::Portuguese);
        assert_eq!(tr!("Baixar agora"), "Baixar agora");
        assert_eq!(trf!("{} de {}", 3, 10), "3 de 10");
    }

    #[test]
    fn texts_in_code() {
        let code = r#"let a = tr!("Um \"texto\""); let b = trf!(
            "Fisga {} dispon\u{ed}vel \u{201c}x\u{201d}",
            v
        ); let c = tr!("longo \
                        assim");"#;
        assert_eq!(texts_in(code), ["Um \"texto\"", "Fisga {} disponível “x”", "longo assim"]);
        assert!(texts_in(r#"include_str!("../i18n/en.txt"); my_tr!("x"); fisga_i18n::tr!("sim")"#) == ["sim"]);
        assert_eq!(placeholders("{} e {}"), 2);
        assert_eq!(placeholders("{1} e {0}"), 2);
        assert_eq!(placeholders("{{nada}}"), 0);
    }
}
