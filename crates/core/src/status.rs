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
    history: Vec<RawHistory>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawHistory {
    date: String,
    leader: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Stats {
    stars: f64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawEffort {
    id: String,
    name: String,
    repo: Option<String>,
    /// The site's own picture for it, a path under `assets/img/`.
    avatar: Option<String>,
    tier: String,
    gta: Milestones,
    groundwork: Option<Groundwork>,
    /// What a private build builds on: effort ids (a list, or one id).
    #[serde(rename = "basedOn")]
    based_on: serde_json::Value,
    /// An owner's call the ladder can't express: the score, set by hand.
    #[serde(rename = "manualScore")]
    manual_score: Option<f64>,
    /// Taken offline (`{"at": …, "note": …}` when set): still listed, no longer ranked.
    nuked: serde_json::Value,
    stats: Option<Stats>,
    highlights: Vec<String>,
    blockers: Vec<String>,
}

/// The parts built before the executable exists, each 0..1; together they fill the first 10%.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(default)]
struct Groundwork {
    loader: f64,
    shaders: f64,
    graphics: f64,
    proven: f64,
}

impl Groundwork {
    fn fraction(&self) -> f64 {
        0.2 * clamp01(self.loader) + 0.28 * clamp01(self.shaders) + 0.32 * clamp01(self.graphics) + 0.2 * clamp01(self.proven)
    }
}

fn clamp01(v: f64) -> f64 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// The tracker's scale, 0 to 100, as PortWatch VI draws it: a ladder of steps up to
/// Playable at 50%, then four tracks (story, rendering, performance, stability) fill the rest.
pub const MILESTONES: &[(&str, &str, f64)] = &[
    ("groundwork", "Preparação", 0.0),
    ("loads", "Exe rodando", 10.0),
    ("boots", "Primeiro frame", 20.0),
    ("menu", "Menu principal", 30.0),
    ("world", "No mundo", 40.0),
    ("playable", "Jogável", 50.0),
    ("complete", "Completo", 100.0),
];

fn milestone_index(key: &str) -> usize {
    MILESTONES.iter().position(|(k, _, _)| *k == key).unwrap_or(0)
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
    /// A word (`unknown`, `constant`, `often`, `sometimes`, `rarely`, `stable`) or, as the
    /// tracker writes it for some efforts now, a fraction from 0 to 1.
    pub stability: Option<Stability>,
}

/// How rarely an effort crashes, as the tracker gives it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Stability {
    Fraction(f64),
    Word(String),
}

impl Stability {
    /// The stability track, 0 to 1, as the site scores it.
    pub fn fraction(&self) -> f64 {
        match self {
            Stability::Fraction(v) => clamp01(*v),
            Stability::Word(w) => match w.as_str() {
                "often" => 0.25,
                "sometimes" => 0.55,
                "rarely" => 0.8,
                "stable" => 1.0,
                _ => 0.0,
            },
        }
    }
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

    /// The score on the tracker's 0-100 scale and the furthest milestone, as the site computes it.
    fn score(&self, groundwork: f64) -> (f64, &'static str) {
        let ladder = [
            (self.loads, "loads", 10.0),
            (self.boots, "boots", 20.0),
            (self.menu, "menu", 30.0),
            (self.world, "world", 40.0),
            (self.playable, "playable", 50.0),
        ];
        let mut score = 10.0 * groundwork;
        let mut reached = "groundwork";
        let mut prev = score;
        for (v, key, at) in ladder {
            let v = clamp01(v);
            if v >= 1.0 {
                score = at;
                prev = at;
                reached = key;
                continue;
            }
            if v > 0.0 {
                score = score.max(prev + v * (at - prev));
            }
            break;
        }
        if reached == "playable" {
            let stability = self.stability.as_ref().map_or(0.0, Stability::fraction);
            let quality = 0.35 * clamp01(self.story) + 0.25 * clamp01(self.render) + 0.2 * clamp01(self.fps / 30.0) + 0.2 * stability;
            score = 50.0 + quality * 50.0;
            if quality >= 1.0 {
                reached = "complete";
            }
        }
        (score, reached)
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
    /// Where it sits on the tracker's 0-100 scale.
    pub score: f64,
    /// The furthest milestone, as a key of [`MILESTONES`].
    pub reached: String,
    /// No public code: a build nobody else can run.
    pub private: bool,
    /// For a private build, the names of the efforts it builds on.
    pub based_on: Vec<String>,
    /// Where its picture is (see [`crate::avatars`]); the window asks for them separately.
    pub avatar_url: Option<String>,
    /// Taken offline: still listed, no longer in the race.
    pub nuked: bool,
}

/// The headline: how close the leading effort is to the whole game.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Overall {
    pub score: f64,
    /// Average of the three best scores.
    pub top3: f64,
    /// The furthest milestone any effort reached, as a key of [`MILESTONES`].
    pub furthest: String,
    /// Id of the leading effort.
    pub leader: Option<String>,
    /// Efforts whose game draws its first frame.
    pub booted: usize,
    pub tracked: usize,
    /// The best efforts, best first, for the markers above the bar: (id, name, score, private).
    pub top: Vec<Marker>,
    /// The leader's score day by day.
    pub history: Vec<Point>,
    /// The milestones, for the bar: (key, label, at).
    pub milestones: Vec<(String, String, f64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Marker {
    pub id: String,
    pub name: String,
    pub score: f64,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Point {
    pub date: String,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub source: String,
    pub updated: Option<String>,
    pub overall: Overall,
    /// Every effort by its score on the site, highest first.
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
    let names: std::collections::HashMap<String, String> = t
        .efforts
        .iter()
        .map(|e| (e.id.clone(), if e.name.is_empty() { e.id.clone() } else { e.name.clone() }))
        .collect();
    let mut efforts: Vec<(f64, f64, bool, Effort)> = t
        .efforts
        .into_iter()
        .filter(|e| !e.id.is_empty())
        .map(|e| {
            let (stage, partial) = e.gta.stage();
            let repo = e.repo.filter(|r| r.contains('/'));
            let backend = backend_of(repo.as_deref());
            let rank = e.gta.rank();
            let (raw, reached) = e.gta.score(e.groundwork.as_ref().map_or(0.0, Groundwork::fraction));
            let score = e.manual_score.filter(|v| v.is_finite()).map_or(raw, |v| v.clamp(0.0, 100.0));
            let score = (score * 10.0).round() / 10.0;
            let based_on = match &e.based_on {
                serde_json::Value::String(id) => vec![id.clone()],
                serde_json::Value::Array(ids) => ids.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                _ => Vec::new(),
            };
            let based_on = based_on.iter().map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone())).collect();
            let name = if e.name.is_empty() { e.id.clone() } else { e.name };
            let avatar_url = crate::avatars::url_for(e.avatar.as_deref(), repo.as_deref());
            let nuked = !matches!(e.nuked, serde_json::Value::Null | serde_json::Value::Bool(false));
            (
                rank,
                e.stats.as_ref().map_or(0.0, |s| s.stars),
                nuked,
                Effort {
                    id: e.id,
                    name,
                    private: repo.is_none(),
                    repo,
                    backend,
                    tier: e.tier,
                    stage: stage.into(),
                    partial,
                    gta: e.gta,
                    highlights: e.highlights,
                    blockers: e.blockers,
                    score,
                    reached: reached.into(),
                    based_on,
                    avatar_url,
                    nuked,
                },
            )
        })
        .collect();

    // The headline counts every effort that is still online, private builds included.
    let mut ranked: Vec<(&Effort, f64)> = efforts.iter().filter(|(_, _, nuked, _)| !nuked).map(|(_, stars, _, e)| (e, *stars)).collect();
    ranked.sort_by(|(a, sa), (b, sb)| b.score.total_cmp(&a.score).then(sb.total_cmp(sa)));
    let top3: Vec<f64> = ranked.iter().take(3).map(|(e, _)| e.score).collect();
    let furthest = ranked.iter().map(|(e, _)| milestone_index(&e.reached)).max().unwrap_or(0);
    let overall = Overall {
        score: ranked.first().map_or(0.0, |(e, _)| e.score),
        top3: if top3.is_empty() { 0.0 } else { (top3.iter().sum::<f64>() / top3.len() as f64 * 10.0).round() / 10.0 },
        furthest: MILESTONES[furthest].0.into(),
        leader: ranked.first().map(|(e, _)| e.id.clone()),
        booted: efforts.iter().filter(|(_, _, _, e)| e.gta.boots >= 1.0).count(),
        tracked: efforts.len(),
        top: ranked
            .iter()
            .take(6)
            .map(|(e, _)| Marker { id: e.id.clone(), name: e.name.clone(), score: e.score, private: e.private })
            .collect(),
        history: t
            .history
            .into_iter()
            .filter_map(|h| h.leader.filter(|v| v.is_finite()).map(|score| Point { date: h.date, score }))
            .collect(),
        milestones: MILESTONES.iter().map(|(k, l, at)| (k.to_string(), l.to_string(), *at)).collect(),
    };

    // The race in the order the site gives it: by score (the percentage shown), then name.
    efforts.sort_by(|(_, _, _, a), (_, _, _, b)| b.score.total_cmp(&a.score).then(a.name.cmp(&b.name)));
    Ok(Report {
        source: SITE_URL.into(),
        updated: t.updated,
        overall,
        efforts: efforts.into_iter().map(|(_, _, _, e)| e).collect(),
        stale: None,
    })
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
    let url = std::env::var("SIXLAUNCHER_TRACKER_URL").unwrap_or_else(|_| TRACKER_URL.into());
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
        {"id": "closed", "name": "Closed", "repo": null, "tier": "tracked", "basedOn": ["marvis"], "avatar": "assets/img/efforts/closed.jpg",
         "gta": {"loads": 1, "boots": 1, "menu": 1, "world": 1, "playable": 1, "render": 0.5, "fps": 4, "stability": "unknown"}},
        {"id": "marvis", "name": "VIctory", "repo": "Marvisak/VIctory", "tier": "tracked",
         "gta": {"loads": 1, "boots": 1, "menu": 1, "world": 1}, "highlights": ["h"], "blockers": ["b"]},
        {"id": "xcl", "name": "XCL", "repo": "xcl-dev/xcl", "tier": "tracked",
         "gta": {"loads": 1, "boots": 1, "menu": 1, "world": 0.5}, "unknownField": 3},
        {"id": "early", "name": "Early", "repo": "a/b", "tier": "watchlist", "gta": {},
         "groundwork": {"loader": 1, "graphics": 0.5}},
        {"id": "gone", "name": "Gone", "repo": "e/f", "tier": "tracked", "nuked": {"at": "2026-11-23", "note": "repo gone"}, "gta": {"loads": 1, "boots": 1}},
        {"id": "mid", "name": "Mid", "repo": "c/d", "tier": "tracked", "gta": {"loads": 1}}
      ],
      "history": [{"date": "2026-11-22", "leader": null}, {"date": "2026-11-23", "leader": 30}, {"date": "2026-11-24", "leader": 57.6}],
      "news": []
    }"#;

    #[test]
    fn by_percentage() {
        let r = parse(SAMPLE).unwrap();
        let scores: Vec<f64> = r.efforts.iter().map(|e| e.score).collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]), "highest first: {scores:?}");
        let by = |id: &str| r.efforts.iter().find(|e| e.id == id).unwrap().clone();
        assert_eq!(by("marvis").backend, Some(BackendKind::Victory));
        assert_eq!(by("xcl").backend, Some(BackendKind::Xcl));
        assert_eq!(by("closed").repo, None);
        assert_eq!(r.updated.as_deref(), Some("2026-11-24T21:00:00Z"));
        assert!(r.efforts.iter().find(|e| e.id == "gone").unwrap().nuked);
        assert!(!r.efforts[0].nuked);
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
    fn scores_like_the_site() {
        let r = parse(SAMPLE).unwrap();
        let by = |id: &str| r.efforts.iter().find(|e| e.id == id).unwrap().clone();
        // Playable, then the tracks: 50 + 50 * (0.25 * 0.5 + 0.2 * 4/30) = 57.6
        assert_eq!((by("closed").score, by("closed").reached.as_str()), (57.6, "playable"));
        assert!(by("closed").private);
        assert_eq!(by("closed").avatar_url.as_deref(), Some("https://raw.githubusercontent.com/4lwayz/portwatch-vi/main/assets/img/efforts/closed.jpg"));
        assert_eq!(by("marvis").avatar_url.as_deref(), Some("https://avatars.githubusercontent.com/Marvisak?s=88"));
        assert_eq!(by("closed").based_on, ["VIctory"]);
        assert_eq!((by("marvis").score, by("marvis").reached.as_str()), (40.0, "world"));
        // Half of the way from Menus (30) to In the world (40).
        assert_eq!((by("xcl").score, by("xcl").reached.as_str()), (35.0, "menu"));
        assert_eq!(by("mid").score, 10.0);
        // Groundwork only: 10% * (0.2 * 1 + 0.32 * 0.5) = 3.6
        assert_eq!((by("early").score, by("early").reached.as_str()), (3.6, "groundwork"));
    }

    #[test]
    fn stability_as_a_fraction() {
        // As the tracker writes it for ZETTROX: a number, not a word.
        let g: Milestones = serde_json::from_str(
            r#"{"loads":1,"boots":1,"menu":1,"world":1,"playable":1,"story":0.1,"render":0.3,"fps":2,"stability":0.8}"#,
        )
        .unwrap();
        assert_eq!(g.stability, Some(Stability::Fraction(0.8)));
        // 50 + 50 * (0.35 * 0.1 + 0.25 * 0.3 + 0.2 * 2/30 + 0.2 * 0.8) = 64.17
        let (score, reached) = g.score(0.0);
        assert_eq!(((score * 100.0).round() / 100.0, reached), (64.17, "playable"));
        assert_eq!(Stability::Word("rarely".into()).fraction(), 0.8);
        assert_eq!(Stability::Fraction(3.0).fraction(), 1.0);
    }

    #[test]
    fn overall_is_the_leader() {
        let o = parse(SAMPLE).unwrap().overall;
        assert_eq!(o.score, 57.6);
        assert_eq!(o.leader.as_deref(), Some("closed"));
        assert_eq!(o.top3, 44.2);
        assert_eq!(o.furthest, "playable");
        assert_eq!((o.booted, o.tracked), (4, 6));
        assert_eq!(o.top.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["closed", "marvis", "xcl", "mid", "early"]);
        assert_eq!(o.history, [Point { date: "2026-11-23".into(), score: 30.0 }, Point { date: "2026-11-24".into(), score: 57.6 }]);
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
