//! The files a gallery (a post, a profile...) holds, found by gallery-dl
//! (see `crate::gallery`), as downloads of their own.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use fisga_i18n::tr;
use fisga_ipc::{AddRequest, Category, Cookie};

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
    /// their own: in its queue, each in its type's folder (inside the site's,
    /// when the gallery has one), with the gallery's address as their page.
    /// A file already in its folder, or in the list, is left out, as
    /// gallery-dl itself skips what it has: a profile fetched again brings
    /// what is new. Returns how many were added.
    pub fn add_gallery_files(self: &Arc<Self>, id: u64, files: Vec<Found>) -> anyhow::Result<usize> {
        let (request, queue, category, mut taken) = {
            let inner = self.inner.lock().unwrap();
            let job = inner.jobs.get(&id).context(tr!("download não encontrado"))?;
            // What the list already holds, by folder and name.
            let taken: HashSet<PathBuf> = inner
                .jobs
                .values()
                .flat_map(|other| {
                    let named = other.request.output_dir.as_ref().zip(other.request.file_name.as_ref());
                    [named.map(|(dir, name)| dir.join(name)), other.info.path.clone()]
                })
                .flatten()
                .collect();
            (job.request.clone(), job.info.queue, job.info.category, taken)
        };
        let settings = self.settings();
        let subfolder = request.subfolder.as_deref().and_then(folder_name);
        let mut added = 0;
        for found in files.into_iter().take(fisga_ipc::MAX_BATCH) {
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
            if let Some(name) = &found.name {
                let path = dir.join(name);
                if fisga_ipc::existing(&path) != fisga_ipc::Existing::Nothing || !taken.insert(path) {
                    continue;
                }
            }
            let file = AddRequest {
                cookies: cookies_for(&found.url, &request.cookies),
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
                Ok(_) => added += 1,
                Err(e) => tracing::warn!(id, error = %e, "um arquivo da galeria ficou de fora"),
            }
        }
        tracing::info!(id, added, "arquivos da galeria na fila");
        Ok(added)
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
}
