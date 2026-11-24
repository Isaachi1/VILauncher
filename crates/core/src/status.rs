//! Where each GTA VI effort stands, from PortWatch VI's tracker.
//!
//! PortWatch VI (<https://portwatch-vi.xyz>, repository `4lwayz/portwatch-vi`) is a
//! static site that follows the public efforts to run the game on PC. Its data is
//! one JSON file, `data/tracker.json`, refreshed every few hours. The launcher only
//! reads it to show where VIctory, XCL and the others are: nothing in it is run,
//! downloaded or installed. Its milestones are the site's editorial estimates, and
//! many are self-reported by the projects.
//!
//! The last good copy is kept in the cache folder, so the tab still shows
//! something offline.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::BackendKind;
use crate::paths;
use crate::{Error, Result};

pub const TRACKER_URL: &str = "https://raw.githubusercontent.com/4lwayz/portwatch-vi/main/data/tracker.json";
pub const SITE_URL: &str = "https://portwatch-vi.xyz";

/// The repositories behind each of our backends, as the tracker names them.
const REPOS: &[(&str, BackendKind)] = &[
    ("marvisak/victory", BackendKind::Victory),
    ("lportvi/victory", BackendKind::Victory),
    ("xcl-dev/xcl", BackendKind::Xcl),
    ("ivall/xcl", BackendKind::Xcl),
    ("lportvi/xcl", BackendKind::Xcl),
];

/// The part of `tracker.json` the launcher reads. Every field is optional, so a
/// change to the site's format drops a field instead of the whole tab.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Tracker {
    updated: Option<String>,
    efforts: Vec<RawEffort>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawEffort {
    id: String,
    name: String,
    repo: Option<String>,
    tier: String,
    gta: Milestones,
    highlights: Vec<String>,
    blockers: Vec<String>,
}

/// 0, 0.5 or 1 for each step; `render` is 0..1 and `fps` a number.
#[derive(Debug, Default, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Milestones {
    pub loads: f64,
    pub boots: f64,
    pub menu: f64,
    pub world: f64,
    pub playable: f64,
    pub story: f64,
    pub render: f64,
    pub fps: f64,
}

impl Milestones {
    /// The furthest step reached, in the order the tracker uses, with whether it
    /// is only partly reached.
    pub fn stage(&self) -> (&'static str, bool) {
        let steps = [
            (self.story, "Story"),
            (self.playable, "Jogável"),
            (self.world, "No mundo"),
            (self.menu, "Menus"),
            (self.boots, "Inicia"),
            (self.loads, "Carrega o executável"),
        ];
        for (v, name) in steps {
            if v > 0.0 {
                return (name, v < 1.0);
            }
        }
        ("Nada roda ainda", false)
    }

    /// The steps summed, for ordering: one more full step always counts more
    /// than any amount of rendering or frame rate.
    fn rank(&self) -> f64 {
        let steps = self.loads + self.boots + self.menu + self.world + self.playable + self.story;
        steps * 1000.0 + self.render * 100.0 + self.fps.min(99.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effort {
    pub id: String,
    pub name: String,
    /// `owner/name` on GitHub; None when the code is not public.
    pub repo: Option<String>,
    /// The backend of ours it is, if any.
    pub backend: Option<BackendKind>,
    /// `tracked` (in the race) or `watchlist`.
    pub tier: String,
    pub stage: String,
    pub partial: bool,
    pub gta: Milestones,
    pub highlights: Vec<String>,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub source: String,
    pub updated: Option<String>,
    /// Ours first (VIctory, XCL), then the others ordered by how far they got.
    pub efforts: Vec<Effort>,
    /// Set when the network failed and this is the cached copy.
    pub stale: Option<String>,
}

fn backend_of(repo: Option<&str>) -> Option<BackendKind> {
    let repo = repo?.to_ascii_lowercase();
    REPOS.iter().find(|(r, _)| *r == repo).map(|(_, k)| *k)
}

/// Turns the tracker's JSON into the report the window shows.
pub fn parse(json: &str) -> Result<Report> {
    let t: Tracker = serde_json::from_str(json).map_err(|e| Error::Config(format!("PortWatch VI: unexpected data: {e}")))?;
    let mut efforts: Vec<(f64, Effort)> = t
        .efforts
        .into_iter()
        .filter(|e| !e.id.is_empty())
        .map(|e| {
            let (stage, partial) = e.gta.stage();
            let repo = e.repo.filter(|r| !r.is_empty());
            let backend = backend_of(repo.as_deref());
            let rank = e.gta.rank();
            let name = if e.name.is_empty() { e.id.clone() } else { e.name };
            (
                rank,
                Effort {
                    id: e.id,
                    name,
                    repo,
                    backend,
                    tier: e.tier,
                    stage: stage.into(),
                    partial,
                    gta: e.gta,
                    highlights: e.highlights,
                    blockers: e.blockers,
                },
            )
        })
        .collect();
    efforts.sort_by(|(ra, a), (rb, b)| {
        (b.backend.is_some(), b.tier == "tracked")
            .cmp(&(a.backend.is_some(), a.tier == "tracked"))
            .then(rb.total_cmp(ra))
            .then(a.name.cmp(&b.name))
    });
    Ok(Report { source: SITE_URL.into(), updated: t.updated, efforts: efforts.into_iter().map(|(_, e)| e).collect(), stale: None })
}

pub fn cache_file() -> PathBuf {
    paths::cache_dir().join("portwatch-tracker.json")
}

/// Fetches the tracker, or falls back to the cached copy when `fetch` fails.
pub fn report_with(fetch: impl FnOnce() -> Result<String>, cache: &Path) -> Result<Report> {
    match fetch().and_then(|body| parse(&body).map(|r| (body, r))) {
        Ok((body, report)) => {
            if let Some(dir) = cache.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(cache, body);
            Ok(report)
        }
        Err(e) => {
            let msg = e.to_string();
            let cached = fs::read_to_string(cache).map_err(|_| Error::Config(msg.clone()))?;
            let mut report = parse(&cached)?;
            report.stale = Some(msg);
            Ok(report)
        }
    }
}

pub fn fetch_tracker() -> Result<String> {
    let url = std::env::var("VILAUNCHER_TRACKER_URL").unwrap_or_else(|_| TRACKER_URL.into());
    crate::net::get_text(&url, Duration::from_secs(20))
}

pub fn report() -> Result<Report> {
    report_with(fetch_tracker, &cache_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A made-up tracker in the site's format (not the site's data).
    const SAMPLE: &str = r#"{
      "updated": "2026-11-24T21:00:00Z",
      "efforts": [
        {"id": "closed", "name": "Closed", "repo": null, "tier": "tracked",
         "gta": {"loads": 1, "boots": 1, "menu": 1, "world": 1, "playable": 1, "render": 0.5, "fps": 4}},
        {"id": "marvis", "name": "VIctory", "repo": "Marvisak/VIctory", "tier": "tracked",
         "gta": {"loads": 1, "boots": 1, "menu": 1, "world": 1}, "highlights": ["h"], "blockers": ["b"]},
        {"id": "xcl", "name": "XCL", "repo": "xcl-dev/xcl", "tier": "tracked",
         "gta": {"loads": 1, "boots": 1, "menu": 1, "world": 0.5}, "unknownField": 3},
        {"id": "early", "name": "Early", "repo": "a/b", "tier": "watchlist", "gta": {}},
        {"id": "mid", "name": "Mid", "repo": "c/d", "tier": "tracked", "gta": {"loads": 1}}
      ],
      "news": []
    }"#;

    #[test]
    fn ours_first_then_by_progress() {
        let r = parse(SAMPLE).unwrap();
        let ids: Vec<_> = r.efforts.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["marvis", "xcl", "closed", "mid", "early"]);
        assert_eq!(r.efforts[0].backend, Some(BackendKind::Victory));
        assert_eq!(r.efforts[1].backend, Some(BackendKind::Xcl));
        assert_eq!(r.efforts[2].repo, None);
        assert_eq!(r.updated.as_deref(), Some("2026-11-24T21:00:00Z"));
    }

    #[test]
    fn stage_is_the_furthest_step() {
        let r = parse(SAMPLE).unwrap();
        let by = |id: &str| r.efforts.iter().find(|e| e.id == id).unwrap().clone();
        assert_eq!((by("xcl").stage.as_str(), by("xcl").partial), ("No mundo", true));
        assert_eq!((by("marvis").stage.as_str(), by("marvis").partial), ("No mundo", false));
        assert_eq!(by("closed").stage, "Jogável");
        assert_eq!(by("early").stage, "Nada roda ainda");
    }

    #[test]
    fn falls_back_to_the_cache() {
        let t = tempfile::tempdir().unwrap();
        let cache = t.path().join("c/tracker.json");
        assert!(report_with(|| Err(Error::Config("offline".into())), &cache).is_err());

        let fresh = report_with(|| Ok(SAMPLE.into()), &cache).unwrap();
        assert_eq!(fresh.stale, None);
        assert!(cache.is_file());

        let old = report_with(|| Err(Error::Config("offline".into())), &cache).unwrap();
        assert_eq!(old.stale.as_deref(), Some("offline"));
        assert_eq!(old.efforts, fresh.efforts);
    }

    #[test]
    fn bad_data_is_an_error_and_is_not_cached() {
        let t = tempfile::tempdir().unwrap();
        let cache = t.path().join("tracker.json");
        assert!(report_with(|| Ok("<html>".into()), &cache).is_err());
        assert!(!cache.exists());
    }
}
