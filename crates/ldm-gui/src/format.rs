//! Text shown in the list and in the "Novo download" window (pt-BR).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use ldm_ipc::{Category, DownloadInfo, Hashing, SourceKind, State, MAP_PARTS};

/// Binary units with a decimal comma: "1,5 MB".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

/// What is going on, for the tray: "2 baixando · 1,5 MB/s · 3 na fila".
pub fn activity(infos: &[DownloadInfo]) -> String {
    let running: Vec<&DownloadInfo> = infos.iter().filter(|i| i.state == State::Running).collect();
    let queued = infos.iter().filter(|i| i.state == State::Queued).count();
    let mut parts = match running.len() {
        0 => vec!["Nenhum download em andamento".to_string()],
        n => {
            let speed: f64 = running.iter().map(|i| i.speed).sum();
            vec![format!("{n} baixando"), format!("{}/s", bytes(speed as u64))]
        }
    };
    if queued > 0 {
        parts.push(format!("{queued} na fila"));
    }
    parts.join(" · ")
}

pub fn size(info: &DownloadInfo) -> String {
    match info.total {
        Some(total) => bytes(total),
        None if info.probing => "verificando…".into(),
        None if info.state == State::Completed => bytes(info.downloaded),
        None => "desconhecido".into(),
    }
}

pub fn fraction(info: &DownloadInfo) -> f64 {
    match info.total {
        _ if info.state == State::Completed => 1.0,
        Some(total) if total > 0 => (info.downloaded as f64 / total as f64).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

pub fn progress_text(info: &DownloadInfo) -> String {
    match info.total {
        _ if info.state == State::Completed => "100%".into(),
        Some(total) if total > 0 => format!("{:.1}%", fraction(info) * 100.0).replace('.', ","),
        _ if info.downloaded > 0 => bytes(info.downloaded),
        _ => String::new(),
    }
}

/// "120,5 MB de 1,2 GB", or just what was downloaded when the size is unknown.
pub fn downloaded(info: &DownloadInfo) -> String {
    match info.total {
        Some(total) if info.state == State::Completed => bytes(total),
        Some(total) => format!("{} de {}", bytes(info.downloaded.min(total)), bytes(total)),
        None => bytes(info.downloaded),
    }
}

/// A card's file map: how much of each stretch of the file is downloaded,
/// 0–1. Without one from the daemon (a site video, a single stream), the
/// progress fills it from the left.
pub fn file_map(info: &DownloadInfo) -> [f64; MAP_PARTS] {
    if info.state == State::Completed {
        return [1.0; MAP_PARTS];
    }
    let mut map = [0.0; MAP_PARTS];
    if info.map.len() == MAP_PARTS {
        for (part, percent) in map.iter_mut().zip(&info.map) {
            *part = f64::from(*percent) / 100.0;
        }
        return map;
    }
    let filled = fraction(info) * MAP_PARTS as f64;
    for (i, part) in map.iter_mut().enumerate() {
        *part = (filled - i as f64).clamp(0.0, 1.0);
    }
    map
}

/// A card's badge: the file's extension ("ISO"), else what it is.
pub fn badge(info: &DownloadInfo) -> String {
    let extension = Path::new(&info.name).extension().and_then(OsStr::to_str).filter(|ext| (1..=5).contains(&ext.len()));
    match (extension, info.kind) {
        (Some(ext), _) => ext.to_uppercase(),
        (None, SourceKind::Direct) => "ARQUIVO".into(),
        (None, _) => "VÍDEO".into(),
    }
}

/// The number in a card's ring: "62%", a check mark when done.
pub fn ring_text(info: &DownloadInfo) -> String {
    match info.total {
        _ if info.state == State::Completed => "\u{2713}".into(),
        Some(total) if total > 0 => format!("{:.0}%", (fraction(info) * 100.0).floor()),
        _ => String::new(),
    }
}

/// Themed icon for a category (sidebar).
pub fn category_icon(category: Category) -> &'static str {
    match category {
        Category::Video => "video-x-generic-symbolic",
        Category::Music => "audio-x-generic-symbolic",
        Category::Image => "image-x-generic-symbolic",
        Category::Document => "x-office-document-symbolic",
        Category::Compressed => "package-x-generic-symbolic",
        Category::Program => "application-x-executable-symbolic",
        Category::Other => "folder-symbolic",
    }
}

/// When it was added, in local time: "23/10/2026 14:05".
pub fn added(info: &DownloadInfo) -> String {
    gtk::glib::DateTime::from_unix_local(info.created_at as i64)
        .and_then(|date| date.format("%d/%m/%Y %H:%M"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// The search box: every word typed must be in the name or the address
/// (case and accents ignored).
pub fn matches_search(info: &DownloadInfo, query: &str) -> bool {
    let fold = |text: &str| -> String {
        text.to_lowercase()
            .chars()
            .map(|c| match c {
                'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
                'é' | 'è' | 'ê' | 'ë' => 'e',
                'í' | 'ì' | 'î' | 'ï' => 'i',
                'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
                'ú' | 'ù' | 'û' | 'ü' => 'u',
                'ç' => 'c',
                other => other,
            })
            .collect()
    };
    let haystack = fold(&format!("{} {}", info.name, info.url));
    fold(query).split_whitespace().all(|word| haystack.contains(word))
}

pub fn speed(info: &DownloadInfo) -> String {
    if info.state == State::Running && info.speed >= 1.0 {
        format!("{}/s", bytes(info.speed as u64))
    } else {
        String::new()
    }
}

pub fn duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3599 => format!("{} min {:02} s", seconds / 60, seconds % 60),
        _ => format!("{} h {:02} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// Estimated time left for a running download with known size.
pub fn remaining(info: &DownloadInfo) -> String {
    match info.total {
        Some(total) if info.state == State::Running && info.speed >= 1.0 && total >= info.downloaded => {
            duration(((total - info.downloaded) as f64 / info.speed).ceil() as u64)
        }
        _ => String::new(),
    }
}

pub fn state_label(state: State) -> &'static str {
    match state {
        State::Pending => "Aguardando confirmação",
        State::Queued => "Na fila",
        State::Running => "Baixando",
        State::Paused => "Pausado",
        State::Completed => "Concluído",
        State::Failed => "Erro",
    }
}

/// The checksum check of a finished download, as its status.
pub fn check_status(info: &DownloadInfo) -> Option<String> {
    match (&info.hash, info.checksum_matches()) {
        (Some(Hashing::Running { done }), _) => Some(match info.total.filter(|total| *total > 0) {
            Some(total) => format!("Conferindo a soma… {}%", (done * 100 / total).min(100)),
            None => format!("Conferindo a soma… {}", bytes(*done)),
        }),
        (Some(Hashing::Failed { .. }), _) => Some("Concluído \u{2014} atenção: não foi possível conferir a soma".into()),
        (Some(Hashing::Done { digest }), Some(false)) => {
            Some(format!("Concluído \u{2014} atenção: o {} não confere", digest.algo.label()))
        }
        (Some(Hashing::Done { .. }), Some(true)) => Some("Concluído · soma conferida".into()),
        _ => None,
    }
}

pub fn status(info: &DownloadInfo) -> String {
    if info.awaiting_url {
        return "Aguardando novo endereço".into();
    }
    if let Some(check) = check_status(info).filter(|_| info.state == State::Completed && info.error.is_none()) {
        return check;
    }
    let status: String = match (&info.error, info.state) {
        (Some(error), State::Failed) => return format!("Erro: {error}"),
        // Finished, but the antivirus flagged it.
        (Some(warning), State::Completed) => return format!("Concluído \u{2014} atenção: {warning}"),
        (_, State::Running) if info.connections > 1 => format!("Baixando ({} conexões)", info.connections),
        (_, state) => state_label(state).into(),
    };
    match info.speed_limit {
        0 => status,
        _ if info.state == State::Completed => status,
        limit => format!("{status} · máx. {}/s", bytes(limit)),
    }
}

/// The name typed in the "Novo download" window, when the user changed it.
pub fn edited_name(suggested: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    (!typed.is_empty() && typed != suggested).then(|| typed.to_string())
}

/// `name` with its extension replaced by `ext` (or added, when it has none).
pub fn with_extension(name: &str, ext: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, current)) if !stem.is_empty() && (1..=5).contains(&current.len()) && !current.contains(' ') => stem,
        _ => name,
    };
    format!("{stem}.{ext}")
}

/// The daemon keeps a pending download in its category's subfolder
/// (`Downloads/Vídeos`); when the name changes type, the folder follows, as
/// the daemon will do when the download is confirmed.
pub fn category_dir(dir: &Path, from: Category, to: Category, category_folders: bool) -> PathBuf {
    if from == to || !category_folders {
        return dir.to_path_buf();
    }
    let base = match from.folder() {
        Some(folder) if dir.file_name() == Some(OsStr::new(folder)) => dir.parent().unwrap_or(dir),
        // A folder the daemon did not pick by category.
        Some(_) => return dir.to_path_buf(),
        None => dir,
    };
    match to.folder() {
        Some(folder) => base.join(folder),
        None => base.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldm_ipc::{Checksum, SourceKind};

    fn info(state: State, downloaded: u64, total: Option<u64>, speed: f64) -> DownloadInfo {
        DownloadInfo {
            id: 1,
            url: "https://example.com/a.iso".into(),
            kind: SourceKind::Direct,
            name: "a.iso".into(),
            path: None,
            state,
            downloaded,
            total,
            speed,
            connections: 8,
            resumable: true,
            error: None,
            created_at: 0,
            dir: None,
            probing: false,
            category: Category::Other,
            formats: Vec::new(),
            queue: 0,
            order: 1,
            page: None,
            awaiting_url: false,
            checksum: None,
            hash: None,
            map: Vec::new(),
            speed_limit: 0,
        }
    }

    #[test]
    fn bytes_use_binary_units_and_decimal_comma() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1,5 KB");
        assert_eq!(bytes(10 * 1024 * 1024), "10,0 MB");
        assert_eq!(bytes(3 * 1024 * 1024 * 1024), "3,0 GB");
    }

    #[test]
    fn size_while_probing_and_unknown() {
        let mut pending = info(State::Pending, 0, None, 0.0);
        pending.probing = true;
        assert_eq!(size(&pending), "verificando…");
        pending.probing = false;
        assert_eq!(size(&pending), "desconhecido");
        pending.total = Some(2048);
        assert_eq!(size(&pending), "2,0 KB");
        assert_eq!(size(&info(State::Completed, 4096, None, 0.0)), "4,0 KB");
    }

    #[test]
    fn progress() {
        let running = info(State::Running, 256, Some(1024), 128.0);
        assert_eq!(fraction(&running), 0.25);
        assert_eq!(progress_text(&running), "25,0%");
        assert_eq!(speed(&running), "128 B/s");
        assert_eq!(remaining(&running), "6 s");
        assert_eq!(status(&running), "Baixando (8 conexões)");

        let unknown = info(State::Running, 3 * 1024 * 1024, None, 0.0);
        assert_eq!(fraction(&unknown), 0.0);
        assert_eq!(progress_text(&unknown), "3,0 MB");
        assert_eq!(speed(&unknown), "");
        assert_eq!(remaining(&unknown), "");

        let done = info(State::Completed, 10, None, 0.0);
        assert_eq!(fraction(&done), 1.0);
        assert_eq!(progress_text(&done), "100%");

        let paused = info(State::Paused, 512, Some(1024), 99.0);
        assert_eq!(speed(&paused), "");
        assert_eq!(status(&paused), "Pausado");
    }

    #[test]
    fn downloaded_amounts() {
        assert_eq!(downloaded(&info(State::Running, 512, Some(2048), 0.0)), "512 B de 2,0 KB");
        assert_eq!(downloaded(&info(State::Running, 1536, None, 0.0)), "1,5 KB");
        assert_eq!(downloaded(&info(State::Completed, 0, Some(2048), 0.0)), "2,0 KB");
    }

    #[test]
    fn tray_activity() {
        assert_eq!(activity(&[]), "Nenhum download em andamento");
        let list = [
            info(State::Running, 0, None, 1024.0),
            info(State::Running, 0, None, 512.0),
            info(State::Queued, 0, None, 0.0),
            info(State::Completed, 0, None, 0.0),
        ];
        assert_eq!(activity(&list), "2 baixando · 1,5 KB/s · 1 na fila");
        assert_eq!(activity(&list[2..]), "Nenhum download em andamento · 1 na fila");
    }

    #[test]
    fn search_and_date() {
        let mut video = info(State::Running, 0, None, 0.0);
        video.name = "Aula de Programação 01.mp4".into();
        video.url = "https://cursos.exemplo.com/a1".into();
        assert!(matches_search(&video, ""));
        assert!(matches_search(&video, "programacao"));
        assert!(matches_search(&video, "AULA  01"));
        assert!(matches_search(&video, "exemplo aula"), "words can match the address");
        assert!(!matches_search(&video, "aula 02"));
        // The day depends on the time zone; the shape does not.
        video.created_at = 86_400 * 365;
        let date = added(&video);
        assert_eq!(date.len(), "01/01/1971 00:00".len(), "{date}");
        assert_eq!(date.matches('/').count(), 2, "{date}");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(5), "5 s");
        assert_eq!(duration(200), "3 min 20 s");
        assert_eq!(duration(3900), "1 h 05 min");
    }

    #[test]
    fn card_texts() {
        let mut running = info(State::Running, 300, Some(800), 10.0);
        running.name = "ubuntu-24.04.iso".into();
        assert_eq!(badge(&running), "ISO");
        assert_eq!(ring_text(&running), "37%");
        // No map from the daemon: filled from the left.
        assert_eq!(file_map(&running), [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        running.map = vec![100, 50, 0, 0, 0, 0, 100, 0];
        assert_eq!(file_map(&running)[1], 0.5);
        assert_eq!(file_map(&running)[6], 1.0);
        let done = info(State::Completed, 800, Some(800), 0.0);
        assert_eq!(file_map(&done), [1.0; MAP_PARTS]);
        assert_eq!(ring_text(&done), "\u{2713}");
        let mut video = info(State::Queued, 0, None, 0.0);
        video.name = "Aula 03 — Async em Rust".into();
        video.kind = SourceKind::Page;
        assert_eq!(badge(&video), "VÍDEO");
        assert_eq!(ring_text(&video), "");
    }

    #[test]
    fn checksum_status() {
        let mut done = info(State::Completed, 4096, Some(4096), 0.0);
        assert_eq!(status(&done), "Concluído");
        done.hash = Some(Hashing::Running { done: 1024 });
        assert_eq!(status(&done), "Conferindo a soma… 25%");
        let digest = Checksum::parse(&"ab".repeat(32)).unwrap();
        done.hash = Some(Hashing::Done { digest: digest.clone() });
        assert_eq!(status(&done), "Concluído", "nothing to compare with");
        done.checksum = Some(digest);
        assert_eq!(status(&done), "Concluído · soma conferida");
        done.checksum = Some(Checksum::parse(&"cd".repeat(32)).unwrap());
        assert_eq!(status(&done), "Concluído \u{2014} atenção: o SHA-256 não confere");
        done.hash = Some(Hashing::Failed { error: "sumiu".into() });
        assert_eq!(status(&done), "Concluído \u{2014} atenção: não foi possível conferir a soma");
    }

    #[test]
    fn failed_status_shows_the_error() {
        let mut failed = info(State::Failed, 0, None, 0.0);
        failed.error = Some("HTTP 404".into());
        assert_eq!(status(&failed), "Erro: HTTP 404");
        failed.awaiting_url = true;
        assert_eq!(status(&failed), "Aguardando novo endereço");
        let mut flagged = info(State::Completed, 0, None, 0.0);
        flagged.error = Some("o ClamAV encontrou Eicar".into());
        assert_eq!(status(&flagged), "Concluído \u{2014} atenção: o ClamAV encontrou Eicar");
        let mut limited = info(State::Paused, 0, None, 0.0);
        limited.speed_limit = 512 * 1024;
        assert_eq!(status(&limited), "Pausado · máx. 512,0 KB/s");
    }

    #[test]
    fn extension_follows_the_quality() {
        assert_eq!(with_extension("Meu vídeo.webm", "m4a"), "Meu vídeo.m4a");
        assert_eq!(with_extension("Meu vídeo", "mp4"), "Meu vídeo.mp4");
        assert_eq!(with_extension("Ep. 1 do podcast", "m4a"), "Ep. 1 do podcast.m4a");
        assert_eq!(with_extension(".webm", "m4a"), ".webm.m4a");
    }

    #[test]
    fn folder_follows_the_category() {
        let videos = Path::new("/home/u/Downloads/Vídeos");
        assert_eq!(
            category_dir(videos, Category::Video, Category::Music, true),
            Path::new("/home/u/Downloads/Músicas")
        );
        assert_eq!(category_dir(videos, Category::Video, Category::Video, true), videos);
        assert_eq!(category_dir(videos, Category::Video, Category::Music, false), videos);
        assert_eq!(category_dir(videos, Category::Video, Category::Other, true), Path::new("/home/u/Downloads"));
        let base = Path::new("/home/u/Downloads");
        assert_eq!(category_dir(base, Category::Other, Category::Program, true), Path::new("/home/u/Downloads/Programas"));
        let chosen = Path::new("/mnt/filmes");
        assert_eq!(category_dir(chosen, Category::Video, Category::Music, true), chosen);
    }

    #[test]
    fn edited_name_only_when_changed() {
        assert_eq!(edited_name("a.iso", "a.iso"), None);
        assert_eq!(edited_name("a.iso", "  a.iso "), None);
        assert_eq!(edited_name("a.iso", ""), None);
        assert_eq!(edited_name("a.iso", "b.iso"), Some("b.iso".into()));
    }
}
