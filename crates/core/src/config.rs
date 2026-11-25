//! The launcher's configuration (`~/.config/sixlauncher/config.toml`).
//!
//! The same structure travels as JSON between the window and the core, so the
//! window edits exactly what the file holds.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    /// VIctory, from VILinux (Grand Theft Auto VI).
    #[default]
    Victory,
    /// XCL, from LinuxCL (GDK titles in general).
    Xcl,
}

impl BackendKind {
    pub fn name(self) -> &'static str {
        match self {
            BackendKind::Victory => "victory",
            BackendKind::Xcl => "xcl",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunnerKind {
    /// A Proton / GE-Proton folder: ships vkd3d-proton. Linux only.
    Proton,
    /// A wine binary: with vkd3d-proton copied into the prefix on Linux, or a Wine with
    /// D3DMetal (CrossOver, the Game Porting Toolkit) on macOS.
    Wine,
    /// No compatibility layer: on Windows the backend's Windows build runs directly, with the
    /// system's own D3D12. Windows only.
    Native,
}

/// Proton on Linux; Wine on macOS, where Proton does not exist; native on Windows.
impl Default for RunnerKind {
    fn default() -> Self {
        if cfg!(windows) {
            RunnerKind::Native
        } else if cfg!(target_os = "macos") {
            RunnerKind::Wine
        } else {
            RunnerKind::Proton
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Console {
    #[default]
    SeriesX,
    SeriesS,
}

impl Console {
    pub fn arg(self) -> &'static str {
        match self {
            Console::SeriesX => "series-x",
            Console::SeriesS => "series-s",
        }
    }
}

/// What VIctory does when the title calls an import it does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unimplemented {
    #[default]
    Trap,
    Zero,
}

impl Unimplemented {
    pub fn arg(self) -> &'static str {
        match self {
            Unimplemented::Trap => "trap",
            Unimplemented::Zero => "zero",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GameSection {
    /// The unpacked folder of a game you own (the one with MicrosoftGame.config).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<PathBuf>,
    /// The title's executable, relative to `dir`; default: from MicrosoftGame.config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
    pub args: Vec<String>,
    /// Check the executable's SHA-256 against the known one (integrity.rs) when the folder
    /// changes and before playing. On by default.
    pub verify: bool,
}

impl Default for GameSection {
    fn default() -> Self {
        GameSection { dir: None, exe: None, args: Vec::new(), verify: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BackendSection {
    pub kind: BackendKind,
    /// victory.exe or xcl.exe: the backend's Windows build.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe: Option<PathBuf>,
    pub console: Console,
    pub unimplemented: Unimplemented,
    /// XCL: DXC's bin/x64 (dxcompiler.dll, dxil.dll).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dxc_dir: Option<PathBuf>,
    /// XCL: a folder with opus.dll.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opus_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerSection {
    pub kind: RunnerKind,
    /// A Proton folder, or the wine binary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<PathBuf>,
    /// Wine: an unpacked vkd3d-proton release.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vkd3d_dir: Option<PathBuf>,
}

/// The vendor preset of variables a run gets (see [`crate::gpu::preset`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuProfile {
    /// The preset of the GPU that runs the game.
    #[default]
    Auto,
    Off,
    Nvidia,
    Amd,
    Intel,
}

impl GpuProfile {
    pub fn name(self) -> &'static str {
        match self {
            GpuProfile::Auto => "auto",
            GpuProfile::Off => "off",
            GpuProfile::Nvidia => "nvidia",
            GpuProfile::Amd => "amd",
            GpuProfile::Intel => "intel",
        }
    }
}

fn auto() -> String {
    crate::gpu::DEVICE_AUTO.into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GpuSection {
    /// Which GPU runs the game: `auto` (a dedicated one when there is one), `system`
    /// (no change), or a PCI address such as `0000:01:00.0`.
    pub device: String,
    pub profile: GpuProfile,
}

impl Default for GpuSection {
    fn default() -> Self {
        GpuSection { device: auto(), profile: GpuProfile::Auto }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PerformanceSection {
    /// MangoHud's overlay (FPS, frame time, GPU / CPU load).
    pub mangohud: bool,
    /// MANGOHUD_CONFIG; default: [`crate::perf::MANGOHUD_DEFAULT`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mangohud_config: Option<String>,
    /// A frame rate cap, through MangoHud.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps_limit: Option<u32>,
    /// Feral's GameMode, through gamemoderun.
    pub gamemode: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DirsSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<PathBuf>,
}

/// Which desktop notifications the window sends (see [`crate::notify`]). All are on until
/// the player turns one off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationsSection {
    /// An effort passes another in PortWatch VI's race.
    pub race: bool,
    /// A new build of VIctory or XCL is published.
    pub runtimes: bool,
    /// A new version of SixLauncher is released.
    pub app: bool,
}

impl Default for NotificationsSection {
    fn default() -> Self {
        NotificationsSection { race: true, runtimes: true, app: true }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub game: GameSection,
    pub backend: BackendSection,
    pub runner: RunnerSection,
    pub dirs: DirsSection,
    pub gpu: GpuSection,
    pub performance: PerformanceSection,
    pub notifications: NotificationsSection,
    /// Extra environment variables for the run (VICTORY_LOG, DXVK_HUD, ...).
    pub env: BTreeMap<String, String>,
}

fn opt(p: &Option<PathBuf>) -> Option<PathBuf> {
    p.as_deref().filter(|p| !p.as_os_str().is_empty()).map(paths::expand)
}

impl Config {
    pub fn game_dir(&self) -> Option<PathBuf> {
        opt(&self.game.dir)
    }

    pub fn backend_exe(&self) -> Option<PathBuf> {
        opt(&self.backend.exe)
    }

    pub fn runner_path(&self) -> Option<PathBuf> {
        opt(&self.runner.path)
    }

    pub fn vkd3d_dir(&self) -> Option<PathBuf> {
        opt(&self.runner.vkd3d_dir)
    }

    pub fn dxc_dir(&self) -> Option<PathBuf> {
        opt(&self.backend.dxc_dir)
    }

    pub fn opus_dir(&self) -> Option<PathBuf> {
        opt(&self.backend.opus_dir)
    }

    pub fn prefix(&self) -> PathBuf {
        opt(&self.runner.prefix).unwrap_or_else(|| {
            paths::data_dir().join(match self.runner.kind {
                RunnerKind::Proton => "compatdata",
                RunnerKind::Wine | RunnerKind::Native => "prefix",
            })
        })
    }

    pub fn data_dir(&self) -> PathBuf {
        opt(&self.dirs.data).unwrap_or_else(|| paths::data_dir().join(self.backend.kind.name()).join("data"))
    }

    pub fn cache_dir(&self) -> PathBuf {
        opt(&self.dirs.cache).unwrap_or_else(|| paths::cache_dir().join(self.backend.kind.name()))
    }

    pub fn from_toml(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|e| Error::Config(format!("configuration: {e}")))
    }

    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        Ok(format!("# SixLauncher configuration. The launcher writes it; edit freely.\n\n{body}"))
    }
}

pub fn load_from(path: &Path) -> Result<Config> {
    if !path.is_file() {
        return Ok(Config::default());
    }
    Config::from_toml(&fs::read_to_string(path)?)
}

pub fn load() -> Result<Config> {
    load_from(&paths::config_file())
}

pub fn save_to(cfg: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, cfg.to_toml()?)?;
    Ok(())
}

pub fn save(cfg: &Config) -> Result<PathBuf> {
    let path = paths::config_file();
    save_to(cfg, &path)?;
    Ok(path)
}

/// The backend a file name suggests: `victory.exe` or `xcl.exe`.
pub fn guess_backend(exe: &Path) -> Option<BackendKind> {
    let name = exe.file_name()?.to_string_lossy().to_lowercase();
    if name.starts_with("victory") {
        Some(BackendKind::Victory)
    } else if name.starts_with("xcl") {
        Some(BackendKind::Xcl)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempHome;

    #[test]
    fn round_trip() {
        let mut cfg = Config::default();
        cfg.game.dir = Some("/g/My \"Game\"".into());
        cfg.game.args = vec!["-a".into(), "b c".into()];
        cfg.backend.kind = BackendKind::Xcl;
        cfg.backend.exe = Some("/b/xcl.exe".into());
        cfg.backend.console = Console::SeriesS;
        cfg.backend.dxc_dir = Some("/dxc".into());
        cfg.runner.kind = RunnerKind::Wine;
        cfg.runner.vkd3d_dir = Some("/v".into());
        cfg.env.insert("VICTORY_LOG".into(), "info".into());
        cfg.gpu = GpuSection { device: "0000:01:00.0".into(), profile: GpuProfile::Nvidia };
        cfg.performance = PerformanceSection { mangohud: true, mangohud_config: None, fps_limit: Some(60), gamemode: true };
        cfg.notifications.race = false;
        let text = cfg.to_toml().unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), cfg);
        assert!(text.contains("console = \"series-s\""), "{text}");
        assert!(text.contains("[gpu]\ndevice = \"0000:01:00.0\"\nprofile = \"nvidia\""), "{text}");
        // A file from before [gpu] and [performance] existed still loads, with the defaults.
        let old = Config::from_toml("[game]\nargs = []\n").unwrap();
        assert_eq!((old.gpu.device.as_str(), old.performance.gamemode), ("auto", false));
    }

    #[test]
    fn bad_values_are_refused() {
        assert!(Config::from_toml("[backend]\nkind = \"rpcs3\"\n").is_err());
        assert!(Config::from_toml("[game\n").is_err());
        assert!(Config::from_toml("[game]\ndirr = \"/x\"\n").is_err());
    }

    #[test]
    fn missing_file_gives_defaults() {
        let h = TempHome::new();
        let cfg = load().unwrap();
        assert_eq!((cfg.backend.kind, cfg.runner.kind), (BackendKind::Victory, RunnerKind::Proton));
        assert_eq!(cfg.data_dir(), h.home().join(".local/share/sixlauncher/victory/data"));
        assert_eq!(cfg.prefix(), h.home().join(".local/share/sixlauncher/compatdata"));
        assert_eq!(cfg.notifications, NotificationsSection { race: true, runtimes: true, app: true });
    }

    #[test]
    fn notifications_default_on_and_keep_the_others() {
        let cfg = Config::from_toml("[notifications]\nruntimes = false\n").unwrap();
        assert_eq!(cfg.notifications, NotificationsSection { race: true, runtimes: false, app: true });
        assert!(Config::from_toml("[notifications]\nnews = true\n").is_err());
    }

    #[test]
    fn guesses_backend_from_name() {
        assert_eq!(guess_backend(Path::new("/x/victory.exe")), Some(BackendKind::Victory));
        assert_eq!(guess_backend(Path::new("/x/XCL.EXE")), Some(BackendKind::Xcl));
        assert_eq!(guess_backend(Path::new("/x/game.exe")), None);
    }
}
