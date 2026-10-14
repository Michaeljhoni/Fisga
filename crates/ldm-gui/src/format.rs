//! Text shown in the list and in the "Novo download" window (pt-BR).

use ldm_ipc::{DownloadInfo, State};

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

pub fn status(info: &DownloadInfo) -> String {
    match (&info.error, info.state) {
        (Some(error), State::Failed) => format!("Erro: {error}"),
        (_, State::Running) if info.connections > 1 => format!("Baixando ({} conexões)", info.connections),
        (_, state) => state_label(state).into(),
    }
}

/// The name typed in the "Novo download" window, when the user changed it.
pub fn edited_name(suggested: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    (!typed.is_empty() && typed != suggested).then(|| typed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldm_ipc::SourceKind;

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
    fn durations() {
        assert_eq!(duration(5), "5 s");
        assert_eq!(duration(200), "3 min 20 s");
        assert_eq!(duration(3900), "1 h 05 min");
    }

    #[test]
    fn failed_status_shows_the_error() {
        let mut failed = info(State::Failed, 0, None, 0.0);
        failed.error = Some("HTTP 404".into());
        assert_eq!(status(&failed), "Erro: HTTP 404");
    }

    #[test]
    fn edited_name_only_when_changed() {
        assert_eq!(edited_name("a.iso", "a.iso"), None);
        assert_eq!(edited_name("a.iso", "  a.iso "), None);
        assert_eq!(edited_name("a.iso", ""), None);
        assert_eq!(edited_name("a.iso", "b.iso"), Some("b.iso".into()));
    }
}
