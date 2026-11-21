//! The files a gallery (a post, a profile...) holds, found by gallery-dl
//! (see `crate::gallery`), as downloads of their own.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Context;
use fisga_i18n::tr;
use fisga_ipc::{AddRequest, Category, Cookie, Response, State};

use super::{auto_dir, folder_name, Manager};
use crate::gallery::Found;

/// Does the cookie `path` cover the address's `path` (RFC 6265, 5.1.4)?
fn path_matches(path: &str, cookie: &str) -> bool {
    path == cookie || path.strip_prefix(cookie).is_some_and(|rest| cookie.ends_with('/') || rest.starts_with('/'))
}

/// Of the cookies the gallery came with (its site's: the login gallery-dl
/// used), the ones the browser would send to `url`. A file on another site
/// (a CDN) gets none of them.
fn cookies_for(url: &str, cookies: &[Cookie]) -> Vec<Cookie> {
    let Ok(url) = url::Url::parse(url) else { return Vec::new() };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let https = url.scheme() == "https";
    cookies
        .iter()
        .filter(|cookie| {
            let domain = cookie.domain.trim_start_matches('.').to_ascii_lowercase();
            let on_host = host == domain || (!cookie.host_only && host.ends_with(&format!(".{domain}")));
            !domain.is_empty() && on_host && (https || !cookie.secure) && path_matches(url.path(), &cookie.path)
        })
        .cloned()
        .collect()
}

impl Manager {
    /// The files gallery-dl found for the gallery `id`, as downloads of
    /// their own: in its queue, where it is, each in its type's folder
    /// (inside the site's, when the gallery has one), with the gallery's
    /// address as their page. What is already done is left out, as
    /// gallery-dl itself skips what it has, so a profile fetched again brings
    /// what is new: a file in its folder, or one the list has (by its folder
    /// and name, or by its address when it has no name). A download of the
    /// file that failed before starting, as signed links do once expired,
    /// gets the new link and goes back to the queue. Returns how many were
    /// added or sent back.
    pub fn add_gallery_files(self: &Arc<Self>, id: u64, files: Vec<Found>) -> anyhow::Result<usize> {
        let (request, queue, category, mut by_path, mut addresses) = {
            let inner = self.inner.lock().unwrap();
            let job = inner.jobs.get(&id).context(tr!("download não encontrado"))?;
            // Where each download of the list goes, with its state.
            let mut by_path: HashMap<PathBuf, (u64, State)> = HashMap::new();
            for other in inner.jobs.values() {
                let named = other.request.output_dir.as_ref().zip(other.request.file_name.as_ref()).map(|(dir, name)| dir.join(name));
                for path in [named, other.info.path.clone()].into_iter().flatten() {
                    by_path.insert(path, (other.info.id, other.info.state));
                }
            }
            let addresses: HashSet<String> = inner.jobs.values().map(|other| other.request.url.clone()).collect();
            (job.request.clone(), job.info.queue, job.info.category, by_path, addresses)
        };
        let settings = self.settings();
        let subfolder = request.subfolder.as_deref().and_then(folder_name);
        let mut added = Vec::new();
        let mut renewed = 0;
        for found in files.into_iter().take(fisga_ipc::MAX_BATCH) {
            // Removed meanwhile: the rest stays out.
            if !self.inner.lock().unwrap().jobs.contains_key(&id) {
                break;
            }
            if !addresses.insert(found.url.clone()) {
                continue;
            }
            let guess = found.name.clone().or_else(|| url::Url::parse(&found.url).ok().as_ref().and_then(fisga_core::filename::from_url));
            let file_category = match Category::detect(guess.as_deref().unwrap_or_default(), found.kind) {
                Category::Other => category,
                known => known,
            };
            let dir = match (&subfolder, &request.output_dir) {
                (Some(folder), _) => auto_dir(&settings, file_category).join(folder),
                (None, Some(dir)) => dir.clone(),
                (None, None) => auto_dir(&settings, file_category),
            };
            let cookies = cookies_for(&found.url, &request.cookies);
            if let Some(name) = &found.name {
                let path = dir.join(name);
                let on_disk = fisga_ipc::existing(&path);
                match by_path.get(&path).copied() {
                    Some((other, State::Failed)) if on_disk == fisga_ipc::Existing::Nothing => {
                        renewed += usize::from(self.renew(other, &found.url, cookies, &request.url));
                        continue;
                    }
                    Some(_) => continue,
                    // A partial file no download holds is another link's: the
                    // engine leaves it alone and takes a free name.
                    None if on_disk == fisga_ipc::Existing::File => continue,
                    None => {}
                }
                by_path.insert(path, (0, State::Queued));
            }
            let file = AddRequest {
                cookies,
                url: found.url,
                kind: found.kind,
                referrer: Some(request.url.clone()),
                page_url: Some(request.url.clone()),
                file_name: found.name,
                output_dir: Some(dir),
                user_agent: request.user_agent.clone(),
                category: Some(file_category),
                queue: Some(queue),
                start: true,
                confirm: false,
                ..Default::default()
            };
            match self.insert(file) {
                Ok(new) => added.push(new),
                Err(e) => tracing::warn!(id, error = %e, "um arquivo da galeria ficou de fora"),
            }
        }
        self.place_after(id, &added);
        tracing::info!(id, added = added.len(), renewed, "arquivos da galeria na fila");
        Ok(added.len() + renewed)
    }

    /// A gallery's file whose download failed (its signed link expired, most
    /// often) gets the link gallery-dl found now, and goes back to the queue.
    fn renew(&self, id: u64, url: &str, cookies: Vec<Cookie>, page: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(job) = inner.jobs.get_mut(&id).filter(|job| job.info.state == State::Failed) else { return false };
        job.request.url = url.to_string();
        job.request.cookies = cookies;
        job.request.referrer = Some(page.to_string());
        job.info.url = url.to_string();
        job.info.state = State::Queued;
        job.info.error = None;
        let _ = self.updates.send(Response::Update { item: job.info.clone() });
        self.dirty.store(true, Ordering::Relaxed);
        tracing::info!(id, "download da galeria com o link novo");
        true
    }

    /// Puts `ids`, new downloads at the end of the queue, right after
    /// `anchor` in its queue (among the unfinished ones), in their order: a
    /// gallery's files take its place, and download before the galleries
    /// sent after it.
    fn place_after(&self, anchor: u64, ids: &[u64]) {
        if ids.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(queue) = inner.jobs.get(&anchor).map(|job| job.info.queue) else { return };
        let mut line: Vec<(i64, u64)> = inner
            .jobs
            .values()
            .filter(|job| job.info.queue == queue && job.info.state != State::Completed && !ids.contains(&job.info.id))
            .map(|job| (job.info.order, job.info.id))
            .collect();
        line.sort();
        let mut orders: Vec<i64> = line.iter().map(|(order, _)| *order).collect();
        orders.extend(ids.iter().filter_map(|id| inner.jobs.get(id).map(|job| job.info.order)));
        orders.sort();
        for i in 1..orders.len() {
            orders[i] = orders[i].max(orders[i - 1] + 1);
        }
        let mut order: Vec<u64> = line.into_iter().map(|(_, id)| id).collect();
        let at = order.iter().position(|id| *id == anchor).map_or(order.len(), |at| at + 1);
        order.splice(at..at, ids.iter().copied());
        for (id, position) in order.into_iter().zip(orders) {
            let job = inner.jobs.get_mut(&id).expect("id from map");
            if job.info.order != position {
                job.info.order = position;
                let _ = self.updates.send(Response::Update { item: job.info.clone() });
            }
        }
        self.dirty.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::manager;
    use super::super::JobResult;
    use super::*;
    use crate::gallery::parse_line;
    use fisga_ipc::{SourceKind, State};

    fn cookie(name: &str, domain: &str, host_only: bool, secure: bool) -> Cookie {
        Cookie {
            name: name.into(),
            value: "v".into(),
            domain: domain.into(),
            path: "/".into(),
            secure,
            host_only,
            expiration_date: None,
        }
    }

    #[test]
    fn a_sites_cookies_stay_on_that_site() {
        let jar = [
            cookie("sessionid", ".instagram.com", false, true),
            cookie("csrftoken", "www.instagram.com", true, false),
            cookie("other", ".example.com", false, false),
        ];
        let names = |url: &str| cookies_for(url, &jar).into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("https://www.instagram.com/p/C1a2b3/"), ["sessionid", "csrftoken"]);
        assert_eq!(names("https://i.instagram.com/api/x"), ["sessionid"], "a host-only cookie is its host's");
        assert!(names("https://scontent.cdninstagram.com/v/t51/1_n.jpg").is_empty(), "not to the CDN");
        assert!(names("https://notinstagram.com/").is_empty());
        assert_eq!(names("http://www.instagram.com/"), ["csrftoken"], "secure cookies only over https");
        assert!(path_matches("/a/b", "/a") && path_matches("/a/", "/a/") && !path_matches("/ab", "/a"));
    }

    #[tokio::test]
    async fn an_instagram_post_queues_every_photo_into_the_sites_folder() {
        let manager = manager();
        let base = manager.settings().download_dir.unwrap();
        // As the browser's gallery sends a post with several photos.
        let ids = manager
            .add_many(vec![AddRequest {
                url: "https://www.instagram.com/p/C1a2b3/".into(),
                kind: SourceKind::Gallery,
                title: Some("Post de @fulano".into()),
                category: Some(Category::Image),
                subfolder: Some("instagram.com".into()),
                page_url: Some("https://www.instagram.com/fulano/".into()),
                user_agent: Some("Mozilla/5.0".into()),
                cookies: vec![cookie("sessionid", ".instagram.com", false, true)],
                start: false,
                ..Default::default()
            }])
            .unwrap();
        let gallery = ids[0];
        assert_eq!(manager.list()[0].kind, SourceKind::Gallery);
        // What gallery-dl prints for it: three photos and a video (the CDN
        // here is this computer's discard port: those downloads fail fast).
        let output = [
            "FISGAFILE\t\"http://127.0.0.1:9/v/t51/1_n.jpg?stp=dst-jpg&_nc_ht=x\"\t\"1_n\"\t\"jpg\"",
            "FISGAFILE\t\"http://127.0.0.1:9/v/t51/2_n.jpg?stp=dst-jpg&_nc_ht=x\"\t\"2_n\"\t\"jpg\"",
            "[instagram][info] carousel",
            "FISGAFILE\t\"http://127.0.0.1:9/v/t51/3_n.webp?stp=x\"\t\"3_n\"\t\"webp\"",
            "FISGAFILE\t\"http://127.0.0.1:9/o1/v/t16/4.mp4?efg=x\"\t\"4\"\t\"mp4\"",
        ];
        let files: Vec<Found> = output.iter().filter_map(|line| parse_line(line)).collect();
        assert_eq!(manager.add_gallery_files(gallery, files.clone()).unwrap(), 4);
        manager.finish(gallery, JobResult::Replaced("4 arquivos na fila.".into()));

        let list = manager.list();
        assert!(list.iter().all(|item| item.id != gallery), "the gallery left the list");
        let photos = base.join("Imagens").join("instagram.com");
        let mut found: Vec<(String, PathBuf, Category)> =
            list.iter().map(|item| (item.name.clone(), item.dir.clone().unwrap(), item.category)).collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            found,
            [
                ("1_n.jpg".into(), photos.clone(), Category::Image),
                ("2_n.jpg".into(), photos.clone(), Category::Image),
                ("3_n.webp".into(), photos.clone(), Category::Image),
                ("4.mp4".into(), base.join("Vídeos").join("instagram.com"), Category::Video),
            ]
        );
        assert!(list.iter().all(|item| item.queue == fisga_ipc::MAIN_QUEUE && item.state != State::Pending));
        {
            let inner = manager.inner.lock().unwrap();
            for job in inner.jobs.values() {
                assert_eq!(job.request.referrer.as_deref(), Some("https://www.instagram.com/p/C1a2b3/"));
                assert_eq!(job.request.user_agent.as_deref(), Some("Mozilla/5.0"));
                assert!(job.request.cookies.is_empty(), "the login is not sent to the CDN");
            }
        }

        // The same post again: nothing new to add.
        let again = manager.add_many(vec![AddRequest {
            url: "https://www.instagram.com/p/C1a2b3/".into(),
            kind: SourceKind::Gallery,
            category: Some(Category::Image),
            subfolder: Some("instagram.com".into()),
            start: false,
            ..Default::default()
        }]);
        let again = again.unwrap()[0];
        assert_eq!(manager.add_gallery_files(again, files).unwrap(), 0);
        // A file already in the folder is not fetched again either.
        std::fs::create_dir_all(&photos).unwrap();
        std::fs::write(photos.join("5_n.jpg"), "x").unwrap();
        let newer = parse_line("FISGAFILE\t\"http://127.0.0.1:9/v/t51/5_n.jpg\"\t\"5_n\"\t\"jpg\"").unwrap();
        assert_eq!(manager.add_gallery_files(again, vec![newer]).unwrap(), 0);
    }

    fn post(manager: &Arc<Manager>, url: &str) -> u64 {
        let request = AddRequest {
            url: url.into(),
            kind: SourceKind::Gallery,
            category: Some(Category::Image),
            subfolder: Some("x.com".into()),
            start: false,
            ..Default::default()
        };
        manager.add_many(vec![request]).unwrap()[0]
    }

    #[tokio::test]
    async fn a_failed_file_gets_the_new_link_and_nameless_ones_come_once() {
        let manager = manager();
        let photos = manager.settings().download_dir.unwrap().join("Imagens").join("x.com");
        let gallery = post(&manager, "https://x.com/fulano/status/1");
        let file = |url: &str, name: Option<&str>| Found { url: url.into(), kind: SourceKind::Direct, name: name.map(Into::into) };
        // Sent before: its signed link expired before it started.
        let failed = manager
            .add(AddRequest {
                url: "http://127.0.0.1:9/media/A.jpg?token=old".into(),
                file_name: Some("A.jpg".into()),
                output_dir: Some(photos.clone()),
                start: false,
                ..Default::default()
            })
            .unwrap();
        manager.update(failed, |info| {
            info.state = State::Failed;
            info.error = Some("HTTP 403".into());
        });
        let found = vec![
            file("http://127.0.0.1:9/media/A.jpg?token=new", Some("A.jpg")),
            file("http://127.0.0.1:9/media/B", None),
            file("http://127.0.0.1:9/media/B", None),
        ];
        assert_eq!(manager.add_gallery_files(gallery, found.clone()).unwrap(), 2, "A renewed, B once");
        let renewed = manager.list().into_iter().find(|item| item.id == failed).unwrap();
        assert_eq!((renewed.state, renewed.url.as_str(), renewed.error), (State::Queued, "http://127.0.0.1:9/media/A.jpg?token=new", None));
        let request = manager.inner.lock().unwrap().jobs[&failed].request.clone();
        assert_eq!(request.url, "http://127.0.0.1:9/media/A.jpg?token=new");
        assert_eq!(request.referrer.as_deref(), Some("https://x.com/fulano/status/1"));
        // Sent again: nothing new, the nameless one included.
        assert_eq!(manager.add_gallery_files(gallery, found).unwrap(), 0);
    }

    #[tokio::test]
    async fn a_galleries_files_take_its_place_in_the_queue() {
        let manager = manager();
        let first = post(&manager, "https://x.com/fulano/status/1");
        let second = post(&manager, "https://x.com/fulano/status/2");
        let found = (1..=3).map(|n| Found { url: format!("http://127.0.0.1:9/{n}.jpg"), kind: SourceKind::Direct, name: Some(format!("{n}.jpg")) });
        assert_eq!(manager.add_gallery_files(first, found.collect()).unwrap(), 3);
        let mut line: Vec<(i64, String)> = manager.list().into_iter().map(|item| (item.order, item.name)).collect();
        line.sort();
        let names: Vec<&str> = line.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(names[1..4], ["1.jpg", "2.jpg", "3.jpg"], "{names:?}: after the first gallery, before the second");
        let orders: HashSet<i64> = line.iter().map(|(order, _)| *order).collect();
        assert_eq!(orders.len(), 5, "every position its own");
        let second_at = manager.list().into_iter().find(|item| item.id == second).unwrap().order;
        assert_eq!(line.last().unwrap().0, second_at);
    }
}
