//! ScarlettPort's play settings, ported from its own launcher (`launcher/src/presets.rs` and
//! `engine.rs` in the ScarlettPort repository): the environment of each preset. Stable is
//! play.ps1's plain run. Pure functions; the paths are already the ones the runtime sees.

use std::path::{Path, PathBuf};

use crate::config::{ScarlettPreset, ScarlettSection};

/// C1, promoted 2026-11-25: the precompiled graphics pipelines and the verified speed switches.
pub const C1: [&str; 9] = [
    "SCARLETT_PRECOMPILED_GRAPHICS",
    "SCARLETT_READFIX",
    "SCARLETT_COPY_BACK_ONLY_READBACK",
    "SCARLETT_NARROW_ROOT",
    "SCARLETT_NARROW_VIEWS",
    "SCARLETT_NARROW_BVH",
    "SCARLETT_PREP_BARRIERS",
    "SCARLETT_PREP_TABLES",
    "SCARLETT_PREP_REUSE",
];

/// The engine's picture and residency settings (since 0.4.6): ambient and sky light, textures
/// shown complete, the threaded list and submission path, and buffers of 64 KB and up kept on
/// the card.
pub const CAND4: [(&str, &str); 18] = [
    ("SCARLETT_AMBIENT_AIR", "1.5"),
    ("SCARLETT_TEXTURE_NO_WAIT", "1"),
    ("SCARLETT_WIDE_BACK", "1"),
    ("SCARLETT_CARD_REHASH_RUNS", "1"),
    ("SCARLETT_JOIN_COPIES", "1"),
    ("SCARLETT_INDIRECT_WINDOW", "1"),
    ("SCARLETT_RELAY_COPY", "1"),
    ("SCARLETT_CARD_STEPS", "2"),
    ("SCARLETT_INDIRECT_BATCH", "2"),
    ("SCARLETT_MAP_THREADS", "1"),
    ("SCARLETT_CARD_AWAKE", "1"),
    ("SCARLETT_SHOW_AFTER", "1"),
    ("SCARLETT_AMBIENT", "0.8"),
    ("SCARLETT_THIN", "1"),
    ("SCARLETT_JOIN_PARTS", "8"),
    ("SCARLETT_UPLOAD_AHEAD", "1"),
    ("SCARLETT_LEVELS_ON_DEMAND", "1"),
    ("SCARLETT_CARD_BUFFERS_MIN_KB", "64"),
];

/// 0.5.0 switches the engine turns on by default; Stable sets them explicitly so an older build
/// behaves the same.
pub const STABLE_050: [(&str, &str); 4] = [
    ("SCARLETT_SAVE_NO_WAIT", "1"),
    ("SCARLETT_CARD_BIG_ROOM", "1"),
    ("SCARLETT_GUARD_SINK", "1"),
    ("SCARLETT_LOADS_WIDE", "1"),
];

/// Look: Stable plus the picture fixes (ambient light in dark rooms, texture and colour fixes,
/// faces, boats that float) and the more accurate shader translation.
pub const LOOK_ON: [(&str, &str); 2] = [("SCARLETT_LOOK", "1"), ("SCARLETT_SHADER_FIXES", "1")];

/// Experimental: Stable plus the GPU step of the A/B plan, the world-entry crash guard and the
/// 0.5.0 load work that booted cleanly but whose gain is inside the boot-to-boot spread.
pub const EXPERIMENTAL_ON: [(&str, &str); 6] = [
    ("SCARLETT_INDIRECT_WINDOW", "1"),
    ("SCARLETT_INDIRECT_KEEP", "1"),
    ("SCARLETT_TABLED_SKIP_FRAMES", "2"),
    ("SCARLETT_LOAD_UNPACED", "1"),
    ("SCARLETT_PREFETCH", "1"),
    ("SCARLETT_PIPELINE_BOOK", "1"),
];

/// play.ps1's environment for a plain run: game, cache, upscaler, watchdog, relay, pipeline
/// wait, log level, precise syncs, then C1, the engine's settings and the 0.5.0 defaults.
pub fn stable_env(game: &str, cache: &str) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("SCARLETT_GAME_DIR", game.to_string()),
        ("SCARLETT_CACHE_DIR", cache.to_string()),
        ("SCARLETT_OWN_UPSCALER", "1".to_string()),
        ("SCARLETT_NO_WATCHDOG", "1".to_string()),
        ("SCARLETT_RELAY", "2".to_string()),
        ("SCARLETT_PIPELINE_WAIT_MS", "0".to_string()),
        ("SCARLETT_LOG", "warn".to_string()),
        ("SCARLETT_SYNC_PRECISE", "1".to_string()),
        ("SCARLETT_READ_SETTLE_PRECISE", "1".to_string()),
    ];
    v.extend(C1.iter().map(|k| (*k, "1".to_string())));
    v.extend(CAND4.iter().map(|(k, x)| (*k, x.to_string())));
    v.extend(STABLE_050.iter().map(|(k, x)| (*k, x.to_string())));
    v
}

/// Sets `k=v`, replacing an earlier value of the same variable.
fn set(env: &mut Vec<(&'static str, String)>, k: &'static str, v: &str) {
    env.retain(|(n, _)| *n != k);
    env.push((k, v.to_string()));
}

/// Custom (ScarlettPort's `world_env`): the fixed play settings, the speed switches and the
/// engine's picture settings, plus the player's ray tracing and stand-in light.
pub fn custom_env(s: &ScarlettSection, game: &str, cache: &str) -> Vec<(&'static str, String)> {
    let flag = |b: bool| if b { "1" } else { "0" };
    let mut v = vec![
        ("SCARLETT_GAME_DIR", game.to_string()),
        ("SCARLETT_CACHE_DIR", cache.to_string()),
        ("SCARLETT_OWN_UPSCALER", "1".to_string()),
        ("SCARLETT_NO_WATCHDOG", "1".to_string()),
        ("SCARLETT_RELAY", "2".to_string()),
        ("SCARLETT_SYNC_PRECISE", "1".to_string()),
        ("SCARLETT_READ_SETTLE_PRECISE", "1".to_string()),
        ("SCARLETT_PIPELINE_WAIT_MS", "0".to_string()),
        ("SCARLETT_RAYTRACING", flag(s.raytracing).to_string()),
        ("SCARLETT_FALLBACK_LIGHT", flag(s.fallback_light).to_string()),
        ("SCARLETT_PAD_PS", "1".to_string()),
    ];
    v.extend(C1.iter().map(|k| (*k, "1".to_string())));
    v.extend(CAND4.iter().map(|(k, x)| (*k, x.to_string())));
    if s.raytracing {
        v.push(("SCARLETT_BVH_BUILDER", "1".to_string()));
    }
    v
}

/// The environment of the chosen preset.
pub fn env(s: &ScarlettSection, game: &str, cache: &str) -> Vec<(&'static str, String)> {
    let extra: &[(&str, &str)] = match s.preset {
        ScarlettPreset::Stable => &[],
        ScarlettPreset::Look => &LOOK_ON,
        ScarlettPreset::Experimental => &EXPERIMENTAL_ON,
        ScarlettPreset::Custom => return custom_env(s, game, cache),
    };
    let mut v = stable_env(game, cache);
    for (k, x) in extra {
        set(&mut v, k, x);
    }
    v
}

/// The cache a preset uses: Experimental gets its own folder beside the usual one (fx-cand2's
/// cache key includes every switch the translator reads).
pub fn cache_for(preset: ScarlettPreset, base: &Path) -> PathBuf {
    match (preset, base.file_name()) {
        (ScarlettPreset::Experimental, Some(n)) => base.with_file_name(format!("{}-experimental", n.to_string_lossy())),
        _ => base.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(e: &'a [(&'static str, String)], k: &str) -> Option<&'a str> {
        e.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str())
    }

    fn once(e: &[(&'static str, String)]) {
        let mut names: Vec<&str> = e.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), e.len(), "each variable once");
    }

    #[test]
    fn presets_add_to_stable() {
        let stable = stable_env("g", "c");
        for (preset, on) in [(ScarlettPreset::Look, &LOOK_ON[..]), (ScarlettPreset::Experimental, &EXPERIMENTAL_ON[..])] {
            let e = env(&ScarlettSection { preset, ..Default::default() }, "g", "c");
            once(&e);
            for (k, v) in on {
                assert_eq!(get(&e, k), Some(*v), "{preset:?} {k}");
            }
            for (k, v) in &stable {
                if !on.iter().any(|(n, _)| n == k) {
                    assert_eq!(get(&e, k), Some(v.as_str()), "{preset:?} keeps Stable's {k}");
                }
            }
        }
        assert_eq!(env(&ScarlettSection::default(), "g", "c"), stable);
    }

    #[test]
    fn custom_follows_the_options() {
        let mut s = ScarlettSection { preset: ScarlettPreset::Custom, ..Default::default() };
        let e = env(&s, "g", "c");
        once(&e);
        assert_eq!(get(&e, "SCARLETT_RAYTRACING"), Some("0"));
        assert_eq!(get(&e, "SCARLETT_BVH_BUILDER"), None);
        for (k, v) in CAND4 {
            assert_eq!(get(&e, k), Some(v), "Custom keeps the engine's settings: {k}");
        }
        s.raytracing = true;
        s.fallback_light = true;
        let e = env(&s, "g", "c");
        assert_eq!((get(&e, "SCARLETT_RAYTRACING"), get(&e, "SCARLETT_BVH_BUILDER"), get(&e, "SCARLETT_FALLBACK_LIGHT")), (Some("1"), Some("1"), Some("1")));
    }

    #[test]
    fn experimental_cache_is_its_own() {
        let base = Path::new("/c/sixlauncher/scarlett");
        assert_eq!(cache_for(ScarlettPreset::Experimental, base), Path::new("/c/sixlauncher/scarlett-experimental"));
        assert_eq!(cache_for(ScarlettPreset::Look, base), base);
    }

    #[test]
    fn stable_matches_play_ps1() {
        let e = stable_env(r"Z:\g", r"Z:\c");
        let get = |k: &str| e.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("SCARLETT_GAME_DIR"), Some(r"Z:\g"));
        assert_eq!(get("SCARLETT_CACHE_DIR"), Some(r"Z:\c"));
        assert_eq!(get("SCARLETT_RELAY"), Some("2"));
        assert_eq!(get("SCARLETT_LOG"), Some("warn"));
        for k in C1 {
            assert_eq!(get(k), Some("1"), "{k}");
        }
        for (k, v) in CAND4.iter().chain(STABLE_050.iter()) {
            assert_eq!(get(k), Some(*v), "{k}");
        }
        let mut names: Vec<&str> = e.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), e.len(), "each variable once");
    }
}
