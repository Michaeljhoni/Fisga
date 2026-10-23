//! IDM's "Refresh download address": a download whose link expired (signed
//! URLs, file hosts) gets a new link to the same file and continues from its
//! `.ldm-part`.
//!
//! Two ways: the user pastes the new link (`refresh_url`), or the download
//! waits for one (`await_url`) and the next download of the same file sent by
//! the browser gives it its link, cookies and headers instead of becoming a
//! new download. "The same file" means the same size, as IDM does.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use ldm_core::RequestSpec;
use ldm_ipc::{AddRequest, DownloadInfo, Response, SourceKind, State};

use super::{request_headers, Manager};
use crate::notify;

/// The new link may come with another ETag (another CDN node, a new
/// signature); the size was checked, so the saved segments are trusted.
fn forget_etag(path: &Path, url: &str) {
    let control_path = ldm_core::control::control_path(path);
    if let Some(mut control) = ldm_core::control::load(&control_path) {
        control.etag = None;
        control.url = url.to_string();
        if let Err(e) = ldm_core::control::save(&control_path, &control) {
            tracing::warn!(path = %control_path.display(), error = %e, "não foi possível atualizar o arquivo de controle");
        }
    }
}

fn can_refresh(info: &DownloadInfo) -> anyhow::Result<()> {
    match info.state {
        State::Completed => bail!("o download #{} já terminou", info.id),
        State::Pending => bail!("o download #{} ainda aguarda confirmação", info.id),
        _ => Ok(()),
    }
}

impl Manager {
    /// Replaces the link of a download with another one to the same file.
    pub async fn refresh_url(self: &Arc<Self>, id: u64, url: String) -> anyhow::Result<()> {
        let parsed = url::Url::parse(&url).context("URL inválida")?;
        if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
            bail!("esquema não suportado: {}", parsed.scheme());
        }
        let (info, request) = {
            let inner = self.inner.lock().unwrap();
            let job = inner.jobs.get(&id).context("download não encontrado")?;
            (job.info.clone(), job.request.clone())
        };
        can_refresh(&info)?;
        let request = AddRequest { url, ..request };
        if request.kind == SourceKind::Direct {
            let spec = RequestSpec { url: request.url.clone(), headers: request_headers(&request) };
            let probe = ldm_core::probe(&self.client, &spec).await.context("o novo endereço não respondeu")?;
            match (info.total, probe.total) {
                (Some(old), Some(new)) if old != new => {
                    bail!("o novo endereço é de outro arquivo: {new} bytes em vez de {old}")
                }
                (Some(_), None) => {
                    bail!("o novo endereço não informa o tamanho, então não dá para saber se é o mesmo arquivo")
                }
                _ => {}
            }
        }
        self.apply_address(id, request).await
    }

    /// Starts (or stops) waiting for the browser to send the same file again.
    pub fn await_url(self: &Arc<Self>, id: u64, waiting: bool) -> anyhow::Result<()> {
        {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            if waiting {
                can_refresh(&job.info)?;
                if job.info.kind != SourceKind::Direct || job.info.total.is_none() {
                    bail!("capturar o endereço só funciona com links diretos de tamanho conhecido; cole o novo endereço");
                }
                // The old link is useless: stop trying it.
                if let Some(cancel) = job.cancel.as_ref().filter(|_| job.info.state == State::Running) {
                    job.requeue = false;
                    cancel.cancel();
                }
                if job.info.state == State::Queued {
                    job.info.state = State::Paused;
                }
            }
            job.info.awaiting_url = waiting;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
        }
        tracing::info!(id, waiting, "aguardando novo endereço");
        self.save();
        Ok(())
    }

    /// A download waiting for a new address that a file of this size and name fits.
    pub(super) fn awaiting_match(&self, total: Option<u64>, name: &str) -> Option<u64> {
        let total = total?;
        let inner = self.inner.lock().unwrap();
        let candidates: Vec<&DownloadInfo> = inner
            .jobs
            .values()
            .map(|job| &job.info)
            .filter(|info| info.awaiting_url && info.total == Some(total))
            .collect();
        candidates.iter().find(|info| info.name == name).or(candidates.first()).map(|info| info.id)
    }

    /// The browser sent the file a download is waiting for (`new`, still
    /// pending): its link, cookies and headers go to `target`, and `new` goes away.
    pub(super) async fn take_address(self: &Arc<Self>, new: u64, target: u64) {
        let Some(sent) = self.inner.lock().unwrap().jobs.remove(&new).map(|job| job.request) else { return };
        let _ = self.updates.send(Response::Removed { id: new });
        let Some(old) = self.inner.lock().unwrap().jobs.get(&target).map(|job| job.request.clone()) else { return };
        let request = AddRequest {
            url: sent.url,
            referrer: sent.referrer.or(old.referrer.clone()),
            page_url: sent.page_url.or(old.page_url.clone()),
            user_agent: sent.user_agent.or(old.user_agent.clone()),
            cookies: sent.cookies,
            headers: sent.headers,
            ..old
        };
        tracing::info!(new, target, "endereço capturado do navegador");
        match self.apply_address(target, request).await {
            Ok(()) => {
                let name = self.inner.lock().unwrap().jobs.get(&target).map(|j| j.info.name.clone()).unwrap_or_default();
                notify::send("Endereço atualizado", &format!("{name} continua de onde parou."));
            }
            Err(e) => tracing::warn!(target, error = %e, "não foi possível usar o endereço capturado"),
        }
    }

    /// Pauses the download if it runs, swaps its link, and resumes it.
    async fn apply_address(self: &Arc<Self>, id: u64, request: AddRequest) -> anyhow::Result<()> {
        if self.state(id) == Some(State::Running) {
            self.pause(id)?;
            for _ in 0..50 {
                if self.state(id) != Some(State::Running) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if self.state(id) == Some(State::Running) {
                bail!("o download não pausou a tempo; tente de novo");
            }
        }
        let url = request.url.clone();
        let path = {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            job.info.url = url.clone();
            job.info.page = request.page_url.clone().or(request.referrer.clone());
            job.info.awaiting_url = false;
            job.info.error = None;
            job.request = request;
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
            job.info.path.clone()
        };
        if let Some(path) = path {
            forget_etag(&path, &url);
        }
        tracing::info!(id, "endereço atualizado");
        self.save();
        self.resume(id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::manager;
    use super::*;

    /// Serves `size` bytes for any path.
    async fn sized_server(size: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/arquivo.iso", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let _ = socket.read(&mut buffer).await;
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n");
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&vec![b'x'; size]).await;
                });
            }
        });
        url
    }

    /// A paused download of 4 bytes (the size as if found when it started).
    async fn paused(manager: &Arc<Manager>) -> u64 {
        let url = sized_server(4).await;
        let id = manager.add(AddRequest { url, start: false, ..Default::default() }).unwrap();
        assert_eq!(manager.state(id), Some(State::Paused));
        id
    }

    fn info(manager: &Arc<Manager>, id: u64) -> DownloadInfo {
        manager.list().into_iter().find(|i| i.id == id).unwrap()
    }

    #[tokio::test]
    async fn a_new_link_must_be_to_the_same_file() {
        let manager = manager();
        let id = paused(&manager).await;
        manager.update(id, |info| info.total = Some(4));
        let other = sized_server(5).await;
        let error = manager.refresh_url(id, other).await.unwrap_err().to_string();
        assert!(error.contains("outro arquivo"), "{error}");
        assert!(manager.refresh_url(id, "não é um endereço".into()).await.is_err());

        let same = sized_server(4).await;
        manager.refresh_url(id, same.clone()).await.unwrap();
        let info = info(&manager, id);
        assert_eq!(info.url, same);
        assert_ne!(info.state, State::Paused, "continues right away");
    }

    #[tokio::test]
    async fn the_browser_gives_its_link_to_the_waiting_download() {
        let manager = manager();
        let id = paused(&manager).await;
        assert!(manager.await_url(id, true).is_err(), "the size is not known yet");
        manager.update(id, |info| info.total = Some(4));
        manager.await_url(id, true).unwrap();
        assert!(info(&manager, id).awaiting_url);

        // Another size is another file: it stays a download of its own.
        let unrelated = manager
            .add(AddRequest { url: sized_server(9).await, confirm: true, ..Default::default() })
            .unwrap();
        let fresh = sized_server(4).await;
        let cookies = serde_json::from_str(r#"[{"name":"sessao","value":"nova"}]"#).unwrap();
        let sent = manager.add(AddRequest { url: fresh.clone(), confirm: true, cookies, ..Default::default() }).unwrap();
        for _ in 0..100 {
            if manager.state(sent).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(manager.state(sent), None, "the browser's download was absorbed");
        assert_eq!(manager.state(unrelated), Some(State::Pending));
        let info = info(&manager, id);
        assert_eq!(info.url, fresh);
        assert!(!info.awaiting_url);
        let cookies = manager.inner.lock().unwrap().jobs[&id].request.cookies.clone();
        assert_eq!(cookies[0].value, "nova", "with the browser's new cookies");
    }

    #[tokio::test]
    async fn resuming_by_hand_stops_waiting() {
        let manager = manager();
        let id = paused(&manager).await;
        manager.update(id, |info| info.total = Some(4));
        manager.await_url(id, true).unwrap();
        manager.resume(id).unwrap();
        assert!(!info(&manager, id).awaiting_url);
    }
}
