//! Desktop notifications: what changed since the launcher last looked.
//!
//! Three things are worth a notification, each with its own switch in the configuration
//! (`[notifications]`, see [`crate::config::NotificationsSection`]):
//!
//! - **race**: an effort passes another in PortWatch VI's race (see [`crate::status`]);
//! - **runtimes**: a new build of VIctory or XCL is published (see [`crate::update`]);
//! - **app**: a new version of SixLauncher is released (see [`crate::selfupdate`]).
//!
//! What was last seen is kept in `~/.local/state/sixlauncher/notifications.json`. The first
//! look only records a baseline, so nobody gets a pile of notifications on the first start,
//! and the baseline moves on even while a switch is off, so turning it back on does not
//! announce old news. This module only decides *what* to say; the window words it in the
//! player's language and shows it.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{BackendKind, NotificationsSection};
use crate::status::Report;
use crate::update::Manifest;
use crate::{paths, selfupdate, update};

/// An effort only counts as passed while it was among this many leaders: below that the
/// order changes too often to be news.
pub const RACE_TOP: usize = 5;

/// Where an effort stood the last time the race was read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    pub id: String,
    pub name: String,
    pub score: f64,
}

/// What the launcher last saw. Every field is optional, so an older or damaged file only
/// means a new baseline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    /// The race's tracked efforts, best first.
    pub race: Option<Vec<Standing>>,
    /// The newest published commit of each backend, by name.
    pub runtimes: BTreeMap<String, String>,
    /// The newest SixLauncher version already announced.
    pub app: Option<String>,
}

/// One notification, for the window to word.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Event {
    /// `name` passed the efforts in `passed` (best first); `leader` when it now leads.
    Overtake { id: String, name: String, score: f64, passed: Vec<String>, leader: bool },
    /// A new build of `backend` is published (`commit`, from `repo`, the tracker's `effort`).
    Runtime { backend: BackendKind, commit: String, repo: Option<String>, effort: Option<String> },
    /// SixLauncher `version` is released; this copy is `current`.
    App { version: String, current: String },
}

/// The race as it stands: the tracked efforts still online, best first.
pub fn standings(report: &Report) -> Vec<Standing> {
    let mut list: Vec<Standing> = report
        .efforts
        .iter()
        .filter(|e| e.tier == "tracked" && !e.nuked)
        .map(|e| Standing { id: e.id.clone(), name: e.name.clone(), score: e.score })
        .collect();
    list.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    list
}

/// Who passed whom between two readings. An effort passes another when it was at or below
/// it and is now strictly above it, and the other one was among the [`RACE_TOP`] leaders.
/// Efforts new to the race pass nobody: they were not in it before.
pub fn overtakes(before: &[Standing], now: &[Standing]) -> Vec<Event> {
    let old = |id: &str| before.iter().position(|s| s.id == id).map(|i| (i, before[i].score));
    let old_leader = before.first().map(|s| s.id.as_str());
    let mut events = Vec::new();
    for (rank, a) in now.iter().enumerate() {
        let Some((_, a_was)) = old(&a.id) else { continue };
        let passed: Vec<String> = now
            .iter()
            .filter(|b| b.id != a.id && a.score > b.score)
            .filter_map(|b| {
                let (b_pos, b_was) = old(&b.id)?;
                (b_pos < RACE_TOP && a_was <= b_was).then(|| b.name.clone())
            })
            .collect();
        if passed.is_empty() {
            continue;
        }
        events.push(Event::Overtake {
            id: a.id.clone(),
            name: a.name.clone(),
            score: a.score,
            passed,
            leader: rank == 0 && old_leader != Some(a.id.as_str()),
        });
    }
    events
}

/// The builds published since the last look. `installed` gives the commit installed for a
/// backend: a build the player already has is no news. A build VirusTotal flagged is never
/// announced (the launcher refuses to install it anyway).
pub fn new_runtimes(seen: &BTreeMap<String, String>, manifest: &Manifest, installed: impl Fn(BackendKind) -> Option<String>) -> Vec<Event> {
    [BackendKind::Victory, BackendKind::Xcl]
        .into_iter()
        .filter_map(|kind| {
            let asset = manifest.asset(kind)?;
            let before = seen.get(kind.name())?;
            let flagged = asset.virustotal.as_ref().is_some_and(|s| s.verdict == "flagged");
            let fresh = *before != asset.commit && installed(kind).as_deref() != Some(asset.commit.as_str());
            (fresh && !flagged).then(|| Event::Runtime {
                backend: kind,
                commit: asset.commit.clone(),
                repo: asset.repo.clone(),
                effort: asset.effort.clone(),
            })
        })
        .collect()
}

/// A newer SixLauncher than `current`, unless `latest` was already announced.
pub fn new_app(seen: Option<&str>, latest: &str, current: &str) -> Option<Event> {
    let latest = latest.trim_start_matches('v');
    let newer = selfupdate::compare(latest, current) == Some(Ordering::Greater);
    (newer && seen != Some(latest)).then(|| Event::App { version: latest.to_string(), current: current.to_string() })
}

/// What was read this time; None for what could not be read (it keeps its baseline).
#[derive(Default)]
pub struct Sources<'a> {
    /// PortWatch VI's race; a copy from the cache (offline) is not news and is left out.
    pub race: Option<&'a Report>,
    pub runtimes: Option<&'a Manifest>,
    /// The commit installed for each backend.
    pub installed: BTreeMap<String, String>,
    /// The newest SixLauncher release and this copy's version.
    pub app: Option<(&'a str, &'a str)>,
}

/// Compares `sources` with `seen`, moves `seen` on, and returns what the switches allow.
pub fn check(settings: &NotificationsSection, seen: &mut Seen, sources: &Sources) -> Vec<Event> {
    let mut events = Vec::new();

    if let Some(report) = sources.race.filter(|r| r.stale.is_none()) {
        let now = standings(report);
        if let Some(before) = seen.race.as_ref().filter(|_| settings.race) {
            events.extend(overtakes(before, &now));
        }
        seen.race = Some(now);
    }

    if let Some(manifest) = sources.runtimes {
        if settings.runtimes {
            events.extend(new_runtimes(&seen.runtimes, manifest, |k| sources.installed.get(k.name()).cloned()));
        }
        for kind in [BackendKind::Victory, BackendKind::Xcl] {
            if let Some(asset) = manifest.asset(kind) {
                seen.runtimes.insert(kind.name().into(), asset.commit.clone());
            }
        }
    }

    if let Some(event) = sources.app.and_then(|(latest, current)| new_app(seen.app.as_deref(), latest, current)) {
        if let Event::App { version, .. } = &event {
            seen.app = Some(version.clone());
        }
        if settings.app {
            events.push(event);
        }
    }
    events
}

pub fn state_file() -> PathBuf {
    paths::state_dir().join("notifications.json")
}

pub fn load_seen(path: &Path) -> Seen {
    fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_seen(path: &Path, seen: &Seen) -> crate::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(seen).map_err(|e| crate::Error::Config(e.to_string()))?;
    fs::write(path, text)?;
    Ok(())
}

/// Reads the race, the runtimes and the newest SixLauncher (each that fails is skipped),
/// and returns what is new for a copy at version `current`. Slow: call it off the UI thread.
pub fn poll(settings: &NotificationsSection, current: &str) -> Vec<Event> {
    let report = crate::status::report().ok();
    let manifest = update::fetch_manifest().ok();
    let app = selfupdate::fetch_manifest().ok().flatten();
    let installed = [BackendKind::Victory, BackendKind::Xcl]
        .into_iter()
        .filter_map(|k| update::installed_commit(k, &update::install_dir(k)).map(|c| (k.name().to_string(), c)))
        .collect();
    let sources = Sources {
        race: report.as_ref(),
        runtimes: manifest.as_ref(),
        installed,
        app: app.as_ref().map(|m| (m.version.as_str(), current)),
    };
    let path = state_file();
    let mut seen = load_seen(&path);
    let before = seen.clone();
    let events = check(settings, &mut seen, &sources);
    if seen != before {
        let _ = save_seen(&path, &seen);
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{Asset, Scan};

    fn st(id: &str, score: f64) -> Standing {
        Standing { id: id.into(), name: id.to_uppercase(), score }
    }

    fn passed(events: &[Event]) -> Vec<(String, Vec<String>, bool)> {
        events
            .iter()
            .map(|e| match e {
                Event::Overtake { id, passed, leader, .. } => (id.clone(), passed.clone(), *leader),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_pass_is_news_and_a_new_leader_says_so() {
        let before = [st("a", 40.0), st("b", 35.0), st("c", 20.0)];
        let now = [st("b", 45.0), st("a", 40.0), st("c", 20.0)];
        assert_eq!(passed(&overtakes(&before, &now)), [("b".into(), vec!["A".into()], true)]);

        // Passing two at once is one notification.
        let now = [st("c", 41.0), st("a", 40.0), st("b", 35.0)];
        assert_eq!(passed(&overtakes(&before, &now)), [("c".into(), vec!["A".into(), "B".into()], true)]);

        // Not the leader yet.
        let now = [st("a", 40.0), st("c", 36.0), st("b", 35.0)];
        assert_eq!(passed(&overtakes(&before, &now)), [("c".into(), vec!["B".into()], false)]);
    }

    #[test]
    fn no_news_without_a_pass() {
        let before = [st("a", 40.0), st("b", 35.0)];
        // Both moved, the order did not.
        assert!(overtakes(&before, &[st("a", 50.0), st("b", 45.0)]).is_empty());
        // A tie is not a pass.
        assert!(overtakes(&before, &[st("a", 40.0), st("b", 40.0)]).is_empty());
        // A newcomer at the top passed nobody: it was not in the race.
        assert!(overtakes(&before, &[st("new", 90.0), st("a", 40.0), st("b", 35.0)]).is_empty());
    }

    #[test]
    fn breaking_a_tie_is_a_pass() {
        let before = [st("a", 40.0), st("b", 40.0)];
        assert_eq!(passed(&overtakes(&before, &[st("b", 42.0), st("a", 40.0)])), [("b".into(), vec!["A".into()], true)]);
    }

    #[test]
    fn only_the_leaders_count() {
        let before: Vec<Standing> = (0..8).map(|i| st(&format!("e{i}"), 80.0 - 10.0 * i as f64)).collect();
        // e7 jumps over e6 and e5 (6th and 7th, not news) and e4 (5th, news).
        let mut now = before.clone();
        now[7].score = 45.0;
        now.sort_by(|a, b| b.score.total_cmp(&a.score));
        assert_eq!(passed(&overtakes(&before, &now)), [("e7".into(), vec!["E4".into()], false)]);
    }

    fn report(efforts: &[(&str, &str, f64, bool)], stale: bool) -> Report {
        let json = format!(
            r#"{{"efforts": [{}]}}"#,
            efforts
                .iter()
                .map(|(id, tier, score, nuked)| format!(
                    r#"{{"id": "{id}", "name": "{id}", "repo": "o/{id}", "tier": "{tier}", "manualScore": {score}, "nuked": {nuked}, "gta": {{}}}}"#
                ))
                .collect::<Vec<_>>()
                .join(",")
        );
        let mut r = crate::status::parse(&json).unwrap();
        if stale {
            r.stale = Some("offline".into());
        }
        r
    }

    #[test]
    fn standings_leave_out_the_watchlist_and_the_offline() {
        let r = report(&[("a", "tracked", 30.0, false), ("w", "watchlist", 90.0, false), ("n", "tracked", 80.0, true), ("b", "tracked", 50.0, false)], false);
        assert_eq!(standings(&r).iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    }

    fn asset(commit: &str, verdict: Option<&str>) -> Asset {
        Asset {
            file: "x.tar.gz".into(),
            sha256: "0".repeat(64),
            size: 1,
            commit: commit.into(),
            repo: Some("o/victory".into()),
            effort: Some("VIctory".into()),
            virustotal: verdict.map(|v| Scan { verdict: v.into(), scanned_at: None, files: vec![], note: None }),
        }
    }

    fn manifest(victory: Option<Asset>, xcl: Option<Asset>) -> Manifest {
        Manifest { schema: 1, published_at: None, run_url: None, victory, xcl, recommended: None, builds: vec![], min_score: None, candidates: vec![], missing: false }
    }

    #[test]
    fn a_new_build_is_news_unless_installed_or_flagged() {
        let seen: BTreeMap<String, String> = [("victory".into(), "old".into()), ("xcl".into(), "x1".into())].into();
        let m = manifest(Some(asset("new", Some("clean"))), Some(asset("x1", None)));
        let events = new_runtimes(&seen, &m, |_| None);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], Event::Runtime { backend: BackendKind::Victory, commit, .. } if commit == "new"));

        assert!(new_runtimes(&seen, &m, |_| Some("new".into())).is_empty());
        let flagged = manifest(Some(asset("new", Some("flagged"))), None);
        assert!(new_runtimes(&seen, &flagged, |_| None).is_empty());
        // No baseline for a backend: nothing to compare with.
        assert!(new_runtimes(&BTreeMap::new(), &m, |_| None).is_empty());
    }

    #[test]
    fn a_new_version_once() {
        assert!(matches!(new_app(None, "v0.1.60", "0.1.57"), Some(Event::App { version, .. }) if version == "0.1.60"));
        assert_eq!(new_app(Some("0.1.60"), "v0.1.60", "0.1.57"), None);
        assert_eq!(new_app(None, "0.1.57", "0.1.57"), None);
        assert_eq!(new_app(None, "0.1.50", "0.1.57"), None);
    }

    #[test]
    fn first_look_is_a_baseline_then_news() {
        let all = NotificationsSection::default();
        let mut seen = Seen::default();
        let r1 = report(&[("a", "tracked", 40.0, false), ("b", "tracked", 30.0, false)], false);
        let m1 = manifest(Some(asset("v1", None)), None);
        let events = check(&all, &mut seen, &Sources { race: Some(&r1), runtimes: Some(&m1), ..Default::default() });
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(seen.runtimes.get("victory").map(String::as_str), Some("v1"));

        let r2 = report(&[("a", "tracked", 40.0, false), ("b", "tracked", 50.0, false)], false);
        let m2 = manifest(Some(asset("v2", None)), None);
        let sources = Sources { race: Some(&r2), runtimes: Some(&m2), app: Some(("0.2.0", "0.1.0")), ..Default::default() };
        let events = check(&all, &mut seen.clone(), &sources);
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                Event::Overtake { .. } => "overtake",
                Event::Runtime { .. } => "runtime",
                Event::App { .. } => "app",
            })
            .collect();
        assert_eq!(kinds, ["overtake", "runtime", "app"]);

        // Each switch silences its own kind, and the baseline still moves on.
        let none = NotificationsSection { race: false, runtimes: false, app: false };
        let mut quiet = seen.clone();
        assert!(check(&none, &mut quiet, &sources).is_empty());
        assert_eq!(quiet.app.as_deref(), Some("0.2.0"));
        assert_eq!(quiet.runtimes.get("victory").map(String::as_str), Some("v2"));
        assert!(check(&all, &mut quiet, &sources).is_empty());

        let only_race = NotificationsSection { race: true, runtimes: false, app: false };
        assert_eq!(check(&only_race, &mut seen.clone(), &sources).len(), 1);
    }

    #[test]
    fn the_cached_race_is_not_news() {
        let all = NotificationsSection::default();
        let mut seen = Seen { race: Some(vec![st("a", 40.0), st("b", 30.0)]), ..Default::default() };
        let stale = report(&[("a", "tracked", 40.0, false), ("b", "tracked", 50.0, false)], true);
        assert!(check(&all, &mut seen, &Sources { race: Some(&stale), ..Default::default() }).is_empty());
        assert_eq!(seen.race.as_ref().unwrap()[0].id, "a");
    }

    #[test]
    fn the_state_file_round_trips_and_survives_damage() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("s/notifications.json");
        assert_eq!(load_seen(&path), Seen::default());
        let seen = Seen { race: Some(vec![st("a", 1.5)]), runtimes: [("xcl".into(), "c".into())].into(), app: Some("0.1.2".into()) };
        save_seen(&path, &seen).unwrap();
        assert_eq!(load_seen(&path), seen);
        fs::write(&path, "{not json").unwrap();
        assert_eq!(load_seen(&path), Seen::default());
    }
}
