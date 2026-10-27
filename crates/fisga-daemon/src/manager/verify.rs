//! Checksums: download pages publish a SHA-256 (or SHA-1/384/512) next to
//! the link, and the finished file is read once and compared with it. IDM has
//! nothing like it; a corrupted or swapped file is otherwise only found when
//! it fails to open or install.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use fisga_ipc::{Checksum, HashAlgo, Hashing, Response, State};
use ring::digest;

use super::Manager;
use crate::notify;

/// How often the progress of a check is broadcast.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// The digest of a file, or `None` when `keep_going` (told how many bytes
/// were read so far) says to stop.
fn digest_file(path: &Path, algo: HashAlgo, mut keep_going: impl FnMut(u64) -> bool) -> std::io::Result<Option<Checksum>> {
    let algorithm = match algo {
        HashAlgo::Sha1 => &digest::SHA1_FOR_LEGACY_USE_ONLY,
        HashAlgo::Sha256 => &digest::SHA256,
        HashAlgo::Sha384 => &digest::SHA384,
        HashAlgo::Sha512 => &digest::SHA512,
    };
    let mut file = std::fs::File::open(path)?;
    let mut context = digest::Context::new(algorithm);
    let mut buffer = vec![0; 1 << 20];
    let mut done = 0u64;
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        context.update(&buffer[..read]);
        done += read as u64;
        if !keep_going(done) {
            return Ok(None);
        }
    }
    let hex = context.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(Some(Checksum { algo, hex }))
}

impl Manager {
    /// `Request::Verify`.
    pub fn verify(self: &Arc<Self>, id: u64, checksum: Option<Checksum>) -> anyhow::Result<()> {
        let path = {
            let mut inner = self.inner.lock().unwrap();
            let job = inner.jobs.get_mut(&id).context("download não encontrado")?;
            if matches!(job.info.hash, Some(Hashing::Running { .. })) {
                bail!("o arquivo já está sendo verificado");
            }
            let finished = job.info.state == State::Completed;
            if !finished && checksum.is_none() {
                bail!("o download ainda não terminou; diga a soma que o arquivo deve ter para conferir quando terminar");
            }
            let path = match (finished, &job.info.path) {
                (true, Some(path)) => Some(path.clone()),
                (true, None) => bail!("o download #{id} não tem arquivo"),
                (false, _) => None,
            };
            if checksum.is_some() {
                job.info.checksum = checksum;
            }
            // Here, under the lock: a second request is refused above.
            if path.is_some() {
                job.info.hash = Some(Hashing::Running { done: 0 });
            }
            let _ = self.updates.send(Response::Update { item: job.info.clone() });
            path
        };
        self.save();
        if let Some(path) = path {
            let manager = self.clone();
            tokio::spawn(async move { manager.check_file(id, &path).await });
        }
        Ok(())
    }

    /// Reads the file and compares it with the download's checksum (with
    /// none, only computes its SHA-256), keeping `DownloadInfo::hash` up to
    /// date. False when it is not the file the page published or could not be
    /// read, which the user is told: nothing else should run on it.
    pub(super) async fn check_file(self: &Arc<Self>, id: u64, path: &Path) -> bool {
        let Some((name, expected)) = ({
            let inner = self.inner.lock().unwrap();
            inner.jobs.get(&id).map(|job| (job.info.name.clone(), job.info.checksum.clone()))
        }) else {
            return false;
        };
        let algo = expected.as_ref().map_or(HashAlgo::Sha256, |checksum| checksum.algo);
        if !self.set_hash(id, Hashing::Running { done: 0 }) {
            return false;
        }
        let started = Instant::now();
        let manager = self.clone();
        let file = path.to_path_buf();
        let result = tokio::task::spawn_blocking(move || {
            let mut shown = Instant::now();
            digest_file(&file, algo, |done| {
                if shown.elapsed() < PROGRESS_EVERY {
                    return true;
                }
                shown = Instant::now();
                // Removed or started again: the file is no longer this one.
                manager.set_hash(id, Hashing::Running { done })
            })
        })
        .await;
        let digest = match result {
            Ok(Ok(Some(digest))) => digest,
            Ok(Ok(None)) => return false,
            Ok(Err(e)) => return self.check_failed(id, &name, format!("não foi possível ler o arquivo: {e}")),
            Err(e) => return self.check_failed(id, &name, format!("a verificação falhou: {e}")),
        };
        let matches = expected.as_ref().map_or(true, |expected| *expected == digest);
        tracing::info!(id, algo = algo.name(), matches, secs = started.elapsed().as_secs_f64(), "arquivo verificado");
        self.set_hash(id, Hashing::Done { digest });
        self.save();
        if !matches {
            notify::send(
                "Arquivo diferente do esperado",
                &format!("{name}\nO {} não confere: o arquivo pode estar corrompido ou ter sido trocado.", algo.label()),
            );
        }
        matches
    }

    fn check_failed(&self, id: u64, name: &str, error: String) -> bool {
        tracing::warn!(id, error, "não foi possível verificar o arquivo");
        notify::send("Não foi possível verificar o arquivo", &format!("{name}\n{error}"));
        self.set_hash(id, Hashing::Failed { error });
        self.save();
        false
    }

    /// Like `update`, but only while the download is still the finished one
    /// being checked; false otherwise.
    fn set_hash(&self, id: u64, hash: Hashing) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(job) = inner.jobs.get_mut(&id).filter(|job| job.info.state == State::Completed) else {
            return false;
        };
        job.info.hash = Some(hash);
        let _ = self.updates.send(Response::Update { item: job.info.clone() });
        self.dirty.store(true, Ordering::Relaxed);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::manager;
    use super::*;
    use fisga_ipc::{AddRequest, DownloadInfo};

    const TEST_SHA256: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn temp_file(tag: &str, data: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fisga-verify-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.iso");
        std::fs::write(&file, data).unwrap();
        file
    }

    #[test]
    fn known_digests() {
        let file = temp_file("digests", b"test");
        let digest = |algo| digest_file(&file, algo, |_| true).unwrap().unwrap().hex;
        assert_eq!(digest(HashAlgo::Sha256), TEST_SHA256);
        assert_eq!(digest(HashAlgo::Sha1), "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3");
        assert_eq!(digest(HashAlgo::Sha512).len(), 128);
        assert_eq!(digest_file(&file, HashAlgo::Sha256, |_| false).unwrap(), None, "stopped");
        assert!(digest_file(&file.with_extension("nada"), HashAlgo::Sha256, |_| true).is_err());
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    /// A finished download of `file`.
    fn finished(manager: &Arc<Manager>, file: &Path) -> u64 {
        let id = manager.add(AddRequest { url: "https://e.com/a.iso".into(), start: false, ..Default::default() }).unwrap();
        manager.update(id, |info| {
            info.state = State::Completed;
            info.path = Some(file.to_path_buf());
        });
        id
    }

    async fn checked(manager: &Manager, id: u64) -> DownloadInfo {
        for _ in 0..200 {
            let info = manager.list().into_iter().find(|info| info.id == id).unwrap();
            if !matches!(info.hash, Some(Hashing::Running { .. })) {
                return info;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the check did not finish");
    }

    #[tokio::test]
    async fn verify_compares_with_the_published_checksum() {
        let manager = manager();
        let file = temp_file("compare", b"test");
        let id = finished(&manager, &file);

        // No checksum: only the SHA-256, nothing to compare.
        manager.verify(id, None).unwrap();
        let info = checked(&manager, id).await;
        assert_eq!(info.hash, Some(Hashing::Done { digest: Checksum::parse(TEST_SHA256).unwrap() }));
        assert_eq!(info.checksum_matches(), None);

        manager.verify(id, Some(Checksum::parse(&"ab".repeat(32)).unwrap())).unwrap();
        assert_eq!(checked(&manager, id).await.checksum_matches(), Some(false));

        // The algorithm follows the checksum given.
        manager.verify(id, Some(Checksum::parse("a94a8fe5ccb19ba61c4c0873d391e987982fbbd3").unwrap())).unwrap();
        let info = checked(&manager, id).await;
        assert_eq!(info.checksum_matches(), Some(true));
        assert_eq!(info.checksum.unwrap().algo, HashAlgo::Sha1);
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn unfinished_downloads_keep_the_checksum_for_later() {
        let manager = manager();
        let id = manager.add(AddRequest { url: "https://e.com/b.iso".into(), start: false, ..Default::default() }).unwrap();
        assert!(manager.verify(id, None).is_err(), "nothing to read yet");
        let checksum = Checksum::parse(TEST_SHA256).unwrap();
        manager.verify(id, Some(checksum.clone())).unwrap();
        let info = manager.list().into_iter().find(|info| info.id == id).unwrap();
        assert_eq!(info.checksum, Some(checksum));
        assert_eq!(info.hash, None);
        assert!(manager.verify(999, None).is_err());
    }

    #[tokio::test]
    async fn a_different_file_stops_the_on_complete_actions() {
        let manager = manager();
        let file = temp_file("complete", b"outro conteudo");
        let id = finished(&manager, &file);
        manager.update(id, |info| info.checksum = Some(Checksum::parse(TEST_SHA256).unwrap()));
        assert!(!manager.check_file(id, &file).await);
        assert_eq!(checked(&manager, id).await.checksum_matches(), Some(false));

        std::fs::write(&file, b"test").unwrap();
        assert!(manager.check_file(id, &file).await);
        std::fs::remove_file(&file).unwrap();
        assert!(!manager.check_file(id, &file).await, "missing file");
        assert!(matches!(checked(&manager, id).await.hash, Some(Hashing::Failed { .. })));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }
}
