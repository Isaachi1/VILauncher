//! ScarlettPort's play settings, ported from its own launcher (`launcher/src/presets.rs` and
//! `engine.rs` in the ScarlettPort repository): the environment of a Stable run, which is
//! play.ps1's for a plain run. Pure functions; the paths are already the ones the runtime sees.

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

#[cfg(test)]
mod tests {
    use super::*;

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
