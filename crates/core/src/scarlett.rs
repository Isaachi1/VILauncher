//! ScarlettPort's play settings, ported from its own launcher (`launcher/src/presets.rs` and
//! `engine.rs` in the ScarlettPort repository): the environment of each preset. Stable is
//! play.ps1's plain run. Pure functions; the paths are already the ones the runtime sees.

use std::path::{Path, PathBuf};

use crate::config::{FsrQuality, KbmHints, ScarlettPreset, ScarlettSection, Upscaler};

/// The repository of the default build: SixLauncher's fork, which the builder builds on every
/// run (its PINNED list) and the window offers first.
pub const DEFAULT_REPO: &str = "SixLauncher/ScarlettPort";

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

/// The environment of the chosen preset (without the features: see [`features_env`]).
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

/// A number as the runtime reads it: `1.5`, `2.0`.
fn num(x: f32) -> String {
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.contains('.') { s.to_string() } else { format!("{s}.0") }
}

/// The features' environment (ScarlettPort's tuning.rs): only the ones away from their default.
/// `fsr` says whether a FidelityFX DLL is there: FSR and frame generation are not asked for
/// without it.
pub fn features_env(s: &ScarlettSection, fsr: bool) -> Vec<(&'static str, String)> {
    let d = ScarlettSection::default();
    let mut v = Vec::new();
    if s.full_textures {
        v.push(("SCARLETT_TEXTURES", "full".to_string()));
    }
    if s.hdr {
        v.push(("SCARLETT_HDR", "1".to_string()));
    }
    if s.pad_ps {
        v.push(("SCARLETT_PAD_PS", "1".to_string()));
    }
    if !s.rumble {
        v.push(("SCARLETT_RUMBLE", "0".to_string()));
    } else if (s.rumble_strength - d.rumble_strength).abs() > 0.001 {
        v.push(("SCARLETT_RUMBLE_STRENGTH", num(s.rumble_strength)));
    }
    match s.kbm_hints {
        KbmHints::Auto => {}
        KbmHints::Off => v.push(("SCARLETT_KBM_HINTS", "off".to_string())),
        KbmHints::Always => v.push(("SCARLETT_KBM_HINTS", "always".to_string())),
    }
    if s.frame_cap > 0 {
        v.push(("SCARLETT_PACE", "1".to_string()));
        v.push(("SCARLETT_PACE_FPS", s.frame_cap.to_string()));
    }
    match s.upscaler {
        Upscaler::Default => {}
        Upscaler::Title => v.push(("SCARLETT_UPSCALER", "title".to_string())),
        Upscaler::Stretch => v.push(("SCARLETT_UPSCALER", "stretch".to_string())),
        Upscaler::Fsr if fsr => {
            let quality = match s.fsr_quality {
                FsrQuality::Native => "native",
                FsrQuality::Quality => "quality",
                FsrQuality::Balanced => "balanced",
                FsrQuality::Performance => "performance",
                FsrQuality::Ultra => "ultra",
            };
            v.push(("SCARLETT_UPSCALER", "fsr".to_string()));
            v.push(("SCARLETT_UPSCALE_QUALITY", quality.to_string()));
            v.push(("SCARLETT_UPSCALE_SHARPNESS", num(s.fsr_sharpness)));
            v.push(("SCARLETT_FRAMEGEN", if s.framegen { "1" } else { "0" }.to_string()));
        }
        Upscaler::Fsr => {}
    }
    if s.overlay {
        v.push(("SCARLETT_PANEL", "1".to_string()));
    }
    let name = player_name(&s.player_name);
    if !name.is_empty() {
        v.push(("SCARLETT_GAMERTAG", name));
    }
    if !s.mouse_look {
        v.push(("SCARLETT_MOUSE_LOOK", "0".to_string()));
    } else if (s.mouse_sensitivity - d.mouse_sensitivity).abs() > 0.001 {
        v.push(("SCARLETT_MOUSE_LOOK", num(s.mouse_sensitivity)));
    }
    v
}

/// The name shown as the console player: printable, trimmed, at most 15 characters (a
/// gamertag's length).
pub fn player_name(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(15).collect()
}

/// FidelityFX DLLs the runtime loads for FSR (the loader, or an API DLL).
pub const FSR_DLLS: [&str; 2] = ["amd_fidelityfx_loader_dx12.dll", "amd_fidelityfx_dx12.dll"];

/// FSR can run: a FidelityFX DLL beside the runtime, or the game's own AMD upscaler
/// (`amd_fidelityfx_upscaler*`) in the game folder, where the engine looks for it.
pub fn fsr_available(runtime_dir: &Path, game_dir: &Path) -> bool {
    FSR_DLLS.iter().any(|d| runtime_dir.join(d).is_file())
        || std::fs::read_dir(game_dir).is_ok_and(|entries| entries.flatten().any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with("amd_fidelityfx_upscaler")))
}

/// The full environment of a run: the preset's, then the features (which win).
pub fn run_env(s: &ScarlettSection, game: &str, cache: &str, fsr: bool) -> Vec<(&'static str, String)> {
    let mut v = env(s, game, cache);
    for (k, x) in features_env(s, fsr) {
        set(&mut v, k, &x);
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
    fn features_at_their_defaults_set_nothing() {
        assert!(features_env(&ScarlettSection::default(), true).is_empty());
        assert_eq!(run_env(&ScarlettSection::default(), "g", "c", true), stable_env("g", "c"));
    }

    #[test]
    fn features_map_to_the_runtime_switches() {
        let s = ScarlettSection {
            full_textures: true,
            hdr: true,
            pad_ps: true,
            rumble_strength: 1.5,
            kbm_hints: KbmHints::Off,
            frame_cap: 30,
            upscaler: Upscaler::Fsr,
            framegen: true,
            overlay: true,
            mouse_sensitivity: 2.0,
            ..Default::default()
        };
        let e = features_env(&s, true);
        assert_eq!(get(&e, "SCARLETT_TEXTURES"), Some("full"));
        assert_eq!(get(&e, "SCARLETT_HDR"), Some("1"));
        assert_eq!(get(&e, "SCARLETT_RUMBLE_STRENGTH"), Some("1.5"));
        assert_eq!(get(&e, "SCARLETT_KBM_HINTS"), Some("off"));
        assert_eq!((get(&e, "SCARLETT_PACE"), get(&e, "SCARLETT_PACE_FPS")), (Some("1"), Some("30")));
        assert_eq!((get(&e, "SCARLETT_UPSCALER"), get(&e, "SCARLETT_UPSCALE_QUALITY")), (Some("fsr"), Some("quality")));
        assert_eq!((get(&e, "SCARLETT_UPSCALE_SHARPNESS"), get(&e, "SCARLETT_FRAMEGEN")), (Some("0.2"), Some("1")));
        assert_eq!((get(&e, "SCARLETT_PANEL"), get(&e, "SCARLETT_MOUSE_LOOK")), (Some("1"), Some("2.0")));
        let without = features_env(&s, false);
        assert!(without.iter().all(|(k, _)| !k.starts_with("SCARLETT_UPSCAL") && *k != "SCARLETT_FRAMEGEN"), "no FSR without its DLL");
        let off = ScarlettSection { rumble: false, mouse_look: false, ..Default::default() };
        assert_eq!(features_env(&off, true), [("SCARLETT_RUMBLE", "0".to_string()), ("SCARLETT_MOUSE_LOOK", "0".to_string())]);
        let mut custom = s.clone();
        custom.preset = ScarlettPreset::Custom;
        once(&run_env(&custom, "g", "c", true));
    }

    #[test]
    fn player_name_is_a_gamertag() {
        assert_eq!(player_name("  Pla\tyer  "), "Player");
        assert_eq!(player_name("a very long player name"), "a very long pla");
        let e = features_env(&ScarlettSection { player_name: " Six ".into(), ..Default::default() }, true);
        assert_eq!(get(&e, "SCARLETT_GAMERTAG"), Some("Six"));
        assert!(features_env(&ScarlettSection { player_name: "  ".into(), ..Default::default() }, true).is_empty());
    }

    #[test]
    fn fsr_found_beside_the_runtime_or_in_the_game() {
        let t = tempfile::tempdir().unwrap();
        let (rt, game) = (t.path().join("rt"), t.path().join("game"));
        std::fs::create_dir_all(&rt).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        assert!(!fsr_available(&rt, &game));
        std::fs::write(game.join("amd_fidelityfx_upscaler_dx12.dll"), "").unwrap();
        assert!(fsr_available(&rt, &game));
        std::fs::remove_file(game.join("amd_fidelityfx_upscaler_dx12.dll")).unwrap();
        std::fs::write(rt.join("amd_fidelityfx_loader_dx12.dll"), "").unwrap();
        assert!(fsr_available(&rt, &game));
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

// -- preparing the shaders -------------------------------------------------------------------
//
// `scarlett prepare --jobs N` (runtimes since 0.5) translates every shader into the cache and
// makes the pipelines earlier runs drew with, with no game window. It prints one line per
// event: `prepare step <n>/<steps> <name>`, `prepare progress <name> <done> <total>` (total 0
// when unknown), `prepare note <text>` and, at the end, `prepare done <seconds>`. A stopped
// run resumes: what is already in the cache is skipped.

/// A line of `scarlett prepare`'s output.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PrepLine {
    Step { n: u32, steps: u32, name: String },
    Progress { name: String, done: u64, total: u64 },
    Note { text: String },
    Done { seconds: f64 },
}

/// Shader programs the title has (`scarlett shaders`, measured): the bar's total while the
/// runtime reports only the count it reached.
pub const SHADERS_TOTAL: u64 = 26_084;

pub fn parse_prep(line: &str) -> Option<PrepLine> {
    // The translation step's own count: `25756 shaders in the cache, 14 s`.
    if let Some((n, _)) = line.trim().split_once(" shaders in the cache") {
        let done: u64 = n.trim().parse().ok()?;
        return Some(PrepLine::Progress { name: "shaders".into(), done, total: SHADERS_TOTAL.max(done) });
    }
    let rest = line.trim().strip_prefix("prepare ")?;
    let (word, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let mut w = rest.split_whitespace();
    match word {
        "step" => {
            let (n, steps) = w.next()?.split_once('/')?;
            Some(PrepLine::Step { n: n.parse().ok()?, steps: steps.parse().ok()?, name: w.next()?.to_string() })
        }
        "progress" => Some(PrepLine::Progress { name: w.next()?.to_string(), done: w.next()?.parse().ok()?, total: w.next()?.parse().ok()? }),
        "note" => Some(PrepLine::Note { text: rest.trim().to_string() }),
        "done" => Some(PrepLine::Done { seconds: w.next()?.parse().ok()? }),
        _ => None,
    }
}

/// Parallel jobs: one per logical core but one (the PC stays usable), and no more than free
/// RAM allows at about 1 GiB each beyond 2 GiB kept free. At least one.
pub fn jobs(cores: usize, free_mb: Option<u64>) -> usize {
    let by_cpu = cores.saturating_sub(1).max(1);
    let by_ram = free_mb.map_or(by_cpu, |f| (f.saturating_sub(2048) / 1024) as usize);
    by_cpu.min(by_ram).max(1)
}

/// MemAvailable from /proc/meminfo, in MiB (None elsewhere).
pub fn free_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb: u64 = text.lines().find_map(|l| l.strip_prefix("MemAvailable:"))?.trim().trim_end_matches("kB").trim().parse().ok()?;
    Some(kb / 1024)
}

/// Translated shaders in the preset's cache (`<cache>/shaders/xx/<rest of hash>`; temporary
/// `.`-files not counted), as ScarlettPort's launcher counted them.
pub fn cached_shaders(cfg: &crate::config::Config) -> u64 {
    let Ok(top) = std::fs::read_dir(prep_cache(cfg).join("shaders")) else { return 0 };
    let mut n = 0;
    for d in top.flatten() {
        if d.file_name().len() != 2 || !d.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(d.path()) {
            n += files.flatten().filter(|f| !f.file_name().to_string_lossy().starts_with('.')).count() as u64;
        }
    }
    n
}

/// After `scarlett prepare` ended without error but its end line did not reach the launcher
/// (output in a Wine console window, or in Proton's log): the cache holds the title's shaders
/// (95% of them or more; some do not translate yet).
pub fn cache_complete(cfg: &crate::config::Config) -> bool {
    cached_shaders(cfg) >= SHADERS_TOTAL * 95 / 100
}

/// The file in the preset's cache that records what it was prepared for.
pub const STAMP: &str = "prepared.txt";

/// What a prepared cache is valid for: this build of the runtime (size and modification time
/// of scarlett.exe), the preset and the cache folder. An update of the runtime prepares again;
/// `prepare` then only does what the new translator changed.
pub fn stamp(cfg: &crate::config::Config) -> Option<String> {
    let exe = cfg.backend_exe()?;
    let meta = std::fs::metadata(&exe).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let preset = serde_json::to_value(cfg.scarlett.preset).ok()?;
    Some(format!("runtime={}-{mtime}\npreset={}\ncache={}\n", meta.len(), preset.as_str()?, prep_cache(cfg).display()))
}

/// The cache the preset's runs use (Experimental has its own).
pub fn prep_cache(cfg: &crate::config::Config) -> PathBuf {
    cache_for(cfg.scarlett.preset, &cfg.cache_dir())
}

/// True when the preset's cache was prepared for this runtime build.
pub fn prepared(cfg: &crate::config::Config) -> bool {
    stamp(cfg).is_some_and(|s| std::fs::read_to_string(prep_cache(cfg).join(STAMP)).is_ok_and(|t| t == s))
}

/// Records that the preset's cache is prepared for this runtime build.
pub fn mark_prepared(cfg: &crate::config::Config) -> crate::Result<()> {
    let s = stamp(cfg).ok_or_else(|| crate::Error::Config("no ScarlettPort runtime to prepare for".into()))?;
    let dir = prep_cache(cfg);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(STAMP), s)?;
    Ok(())
}

#[cfg(test)]
mod prep_tests {
    use super::*;
    use crate::config::{BackendKind, Config};

    #[test]
    fn prepare_lines() {
        assert_eq!(parse_prep("prepare step 1/2 shaders"), Some(PrepLine::Step { n: 1, steps: 2, name: "shaders".into() }));
        assert_eq!(parse_prep("prepare progress library 1000 5200"), Some(PrepLine::Progress { name: "library".into(), done: 1000, total: 5200 }));
        assert_eq!(parse_prep("prepare note no pipeline records yet: runs write them"), Some(PrepLine::Note { text: "no pipeline records yet: runs write them".into() }));
        assert_eq!(parse_prep("prepare done 171.4"), Some(PrepLine::Done { seconds: 171.4 }));
        assert_eq!(parse_prep("info: loading"), None);
        assert_eq!(parse_prep("25756 shaders in the cache, 14 s"), Some(PrepLine::Progress { name: "shaders".into(), done: 25756, total: SHADERS_TOTAL }));
        assert_eq!(parse_prep("26090 shaders in the cache, 3 do not translate, 171 s"), Some(PrepLine::Progress { name: "shaders".into(), done: 26090, total: 26090 }));
        assert_eq!(parse_prep("prepare progress shaders x 1"), None);
    }

    #[test]
    fn jobs_leave_a_core_and_fit_in_memory() {
        assert_eq!(jobs(8, None), 7);
        assert_eq!(jobs(8, Some(6144)), 4);
        assert_eq!(jobs(1, Some(512)), 1);
    }

    #[test]
    fn stamp_follows_runtime_and_preset() {
        let h = crate::testutil::TempHome::new();
        let mut cfg = Config::default();
        cfg.backend.kind = BackendKind::Scarlett;
        assert!(!prepared(&cfg), "no runtime: never prepared");
        assert!(mark_prepared(&cfg).is_err());
        let exe = h.path().join("rt/scarlett.exe");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "MZ").unwrap();
        cfg.backend.exe = Some(exe.clone());
        assert!(!prepared(&cfg));
        mark_prepared(&cfg).unwrap();
        assert!(prepared(&cfg));
        cfg.scarlett.preset = ScarlettPreset::Look;
        assert!(!prepared(&cfg), "another preset");
        cfg.scarlett.preset = ScarlettPreset::Stable;
        std::fs::write(&exe, "MZ new build").unwrap();
        assert!(!prepared(&cfg), "another runtime build");
        assert!(!cache_complete(&cfg));
        let shaders = prep_cache(&cfg).join("shaders");
        for i in 0..SHADERS_TOTAL * 95 / 100 {
            let d = shaders.join(format!("{:02x}", i % 256));
            if i < 256 {
                std::fs::create_dir_all(&d).unwrap();
            }
            std::fs::write(d.join(format!("{i}")), "").unwrap();
        }
        std::fs::write(shaders.join("00/.tmp-x"), "").unwrap();
        assert_eq!(cached_shaders(&cfg), SHADERS_TOTAL * 95 / 100);
        assert!(cache_complete(&cfg));
    }
}

// -- saves -----------------------------------------------------------------------------------
//
// The runtime keeps each title's saves in SCARLETT_DATA_DIR (`<data>/<title id>/...`), which
// SixLauncher points at its data folder, outside the prefix. Before each run they are copied to
// `save-backups/<date>` beside it; the runtime's `temp` folders are left out.

/// Backups kept: the newest ones.
pub const KEEP_BACKUPS: usize = 10;

pub fn backups_dir(cfg: &crate::config::Config) -> PathBuf {
    let data = cfg.data_dir();
    data.parent().map_or_else(|| data.join("save-backups"), |p| p.join("save-backups"))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    let mut n = 0;
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let kind = e.file_type()?;
        if kind.is_dir() {
            if e.file_name().eq_ignore_ascii_case("temp") {
                continue;
            }
            std::fs::create_dir_all(&dst)?;
            n += copy_tree(&src, &dst)?;
        } else if kind.is_file() {
            std::fs::copy(&src, &dst)?;
            n += 1;
        }
    }
    Ok(n)
}

/// Copies the saves into `root/<stamp>` and keeps the newest [`KEEP_BACKUPS`]. None when there
/// is nothing to copy yet (no run has saved).
pub fn backup_saves_to(data: &Path, root: &Path, stamp: &str) -> crate::Result<Option<PathBuf>> {
    if !data.is_dir() || std::fs::read_dir(data)?.next().is_none() {
        return Ok(None);
    }
    let dest = root.join(stamp);
    std::fs::create_dir_all(&dest)?;
    if copy_tree(data, &dest)? == 0 {
        std::fs::remove_dir_all(&dest)?;
        return Ok(None);
    }
    let mut all: Vec<PathBuf> = std::fs::read_dir(root)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    all.sort();
    let extra = all.len().saturating_sub(KEEP_BACKUPS);
    for old in &all[..extra] {
        std::fs::remove_dir_all(old)?;
    }
    Ok(Some(dest))
}

/// Before a ScarlettPort run: the saves backup, when it is on. What it did, for the log.
pub fn before_run(cfg: &crate::config::Config) -> Option<String> {
    if cfg.backend.kind != crate::config::BackendKind::Scarlett || !cfg.scarlett.backup_saves {
        return None;
    }
    let stamp = crate::report::stamp(std::time::SystemTime::now());
    Some(match backup_saves_to(&cfg.data_dir(), &backups_dir(cfg), &stamp) {
        Ok(Some(dest)) => format!("saves backed up to {}", dest.display()),
        Ok(None) => return None,
        Err(e) => format!("saves backup failed: {e}"),
    })
}

#[cfg(test)]
mod saves_tests {
    use super::*;

    #[test]
    fn backups_copy_the_saves_and_keep_the_newest() {
        let t = tempfile::tempdir().unwrap();
        let (data, root) = (t.path().join("data"), t.path().join("save-backups"));
        assert_eq!(backup_saves_to(&data, &root, "a").unwrap(), None, "nothing saved yet");
        std::fs::create_dir_all(data.join("4D4F5049/saves")).unwrap();
        std::fs::create_dir_all(data.join("4D4F5049/temp")).unwrap();
        std::fs::write(data.join("4D4F5049/saves/slot0"), "save").unwrap();
        std::fs::write(data.join("4D4F5049/temp/big.bin"), "scratch").unwrap();
        let first = backup_saves_to(&data, &root, "20261127-100000").unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(first.join("4D4F5049/saves/slot0")).unwrap(), "save");
        assert!(!first.join("4D4F5049/temp").exists(), "temp is not a save");
        for i in 1..=KEEP_BACKUPS {
            backup_saves_to(&data, &root, &format!("20261127-1{i:05}")).unwrap();
        }
        let left: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().collect();
        assert_eq!(left.len(), KEEP_BACKUPS);
        assert!(!first.exists(), "the oldest goes first");
    }
}
