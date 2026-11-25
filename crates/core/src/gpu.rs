//! The GPUs of the machine, which one runs the game, and each vendor's preset.
//!
//! The GPUs come from sysfs (`/sys/class/drm/card*/device`): PCI vendor and device
//! ids, the kernel driver, whether the firmware booted on it (`boot_vga`) and, for
//! amdgpu, its VRAM. Names come from the system's `pci.ids` when it has one.
//!
//! On a hybrid laptop (an Intel or AMD iGPU that drives the screen plus an NVIDIA or
//! AMD dGPU) Wine picks the first Vulkan device, often the iGPU. The launcher steers
//! every layer to the GPU chosen in `[gpu] device`:
//!
//! * NVIDIA (proprietary driver) as the render GPU: PRIME render offload,
//!   `__NV_PRIME_RENDER_OFFLOAD=1`, `__GLX_VENDOR_LIBRARY_NAME=nvidia` and
//!   `__VK_LAYER_NV_optimus=NVIDIA_only`;
//! * a Mesa GPU (AMD, Intel, nouveau) that is not the boot GPU: `DRI_PRIME=pci-…`;
//! * any choice on a machine with more than one GPU: Mesa's device-select layer,
//!   `MESA_VK_DEVICE_SELECT=vendor:device!`, which leaves only that GPU visible to
//!   Vulkan (vkd3d-proton), and `__VK_LAYER_NV_optimus=non_NVIDIA_only` to hide an
//!   NVIDIA GPU that was not chosen.
//!
//! A preset adds the variables each vendor's driver documents for large D3D12 titles
//! (see [`preset`]). Variables in `[env]` always win over both.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::{Config, GpuProfile};

pub const VENDOR_NVIDIA: u16 = 0x10de;
pub const VENDOR_AMD: u16 = 0x1002;
pub const VENDOR_INTEL: u16 = 0x8086;

/// `[gpu] device` values that are not a PCI address.
pub const DEVICE_AUTO: &str = "auto";
pub const DEVICE_SYSTEM: &str = "system";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    pub fn from_id(id: u16) -> Self {
        match id {
            VENDOR_NVIDIA => Vendor::Nvidia,
            VENDOR_AMD => Vendor::Amd,
            VENDOR_INTEL => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other => "GPU",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gpu {
    /// The PCI address, `0000:01:00.0`: what `[gpu] device` holds.
    pub slot: String,
    pub vendor: Vendor,
    pub vendor_id: u16,
    pub device_id: u16,
    /// From pci.ids, or the vendor and the device id.
    pub name: String,
    /// The kernel driver: nvidia, amdgpu, i915, xe, nouveau, radeon…
    pub driver: String,
    /// The GPU the firmware started on, usually the one driving the built-in screen.
    pub boot_vga: bool,
    /// An integrated GPU (shares the system's memory), by the heuristics in [`is_integrated`].
    pub integrated: bool,
}

impl Gpu {
    /// `10de:25a2`, as Mesa's device-select layer and DRI_PRIME take it.
    pub fn ids(&self) -> String {
        format!("{:04x}:{:04x}", self.vendor_id, self.device_id)
    }

    /// The proprietary NVIDIA driver (PRIME offload) rather than a Mesa driver.
    pub fn nvidia_blob(&self) -> bool {
        self.driver == "nvidia"
    }

    /// `pci-0000_01_00_0`: the DRI_PRIME form of the address.
    pub fn dri_prime_tag(&self) -> String {
        format!("pci-{}", self.slot.replace([':', '.'], "_"))
    }
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_owned())
}

fn hex_id(s: &str) -> Option<u16> {
    u16::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok()
}

/// Integrated or dedicated, from what sysfs tells: an Intel GPU on PCI bus 0 is the
/// CPU's (Arc cards sit behind a bridge); an amdgpu with less than 2 GiB of VRAM is an
/// APU's carve-out. Anything else counts as dedicated.
pub fn is_integrated(vendor: Vendor, slot: &str, vram: Option<u64>) -> bool {
    let bus = slot.split(':').nth(1).unwrap_or("");
    match vendor {
        Vendor::Intel => bus == "00",
        Vendor::Amd => vram.is_some_and(|v| v < 2 << 30),
        _ => false,
    }
}

/// The name of `vendor:device` in a pci.ids file.
pub fn pci_name(ids: &str, vendor: u16, device: u16) -> Option<String> {
    let (v, d) = (format!("{vendor:04x}"), format!("{device:04x}"));
    let mut in_vendor = false;
    for line in ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None;
            }
            in_vendor = line.get(..4) == Some(v.as_str());
        } else if in_vendor && !line.starts_with("\t\t") {
            let l = &line[1..];
            if l.get(..4) == Some(d.as_str()) {
                let name = l[4..].trim();
                // "GA107M [GeForce RTX 3050 Mobile]": the marketing name is in brackets.
                let shown = match (name.find('['), name.rfind(']')) {
                    (Some(a), Some(b)) if b > a => &name[a + 1..b],
                    _ => name,
                };
                return Some(shown.to_owned());
            }
        }
    }
    None
}

fn pci_ids() -> Option<String> {
    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids", "/usr/share/pci.ids", "/run/host/usr/share/hwdata/pci.ids"]
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())
}

/// The GPUs under a sysfs root (`/sys` on a real machine), boot GPU first.
pub fn list_in(sys: &Path, ids: Option<&str>) -> Vec<Gpu> {
    let Ok(rd) = fs::read_dir(sys.join("class/drm")) else { return Vec::new() };
    let mut cards: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-')))
        .collect();
    cards.sort();
    let mut gpus: Vec<Gpu> = Vec::new();
    for card in cards {
        let dev = card.join("device");
        let (Some(vendor_id), Some(device_id)) =
            (read_trim(&dev.join("vendor")).and_then(|s| hex_id(&s)), read_trim(&dev.join("device")).and_then(|s| hex_id(&s)))
        else {
            continue;
        };
        let slot = read_trim(&dev.join("uevent"))
            .and_then(|u| u.lines().find_map(|l| l.strip_prefix("PCI_SLOT_NAME=").map(str::to_owned)))
            .or_else(|| fs::canonicalize(&dev).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())))
            .unwrap_or_default();
        if slot.is_empty() || gpus.iter().any(|g| g.slot == slot) {
            continue;
        }
        let driver = fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let vendor = Vendor::from_id(vendor_id);
        let vram = read_trim(&dev.join("mem_info_vram_total")).and_then(|s| s.parse().ok());
        let name = ids
            .and_then(|t| pci_name(t, vendor_id, device_id))
            .map(|n| if n.to_lowercase().contains(&vendor.label().to_lowercase()) { n } else { format!("{} {n}", vendor.label()) })
            .unwrap_or_else(|| format!("{} {vendor_id:04x}:{device_id:04x}", vendor.label()));
        gpus.push(Gpu {
            boot_vga: read_trim(&dev.join("boot_vga")).as_deref() == Some("1"),
            integrated: is_integrated(vendor, &slot, vram),
            slot,
            vendor,
            vendor_id,
            device_id,
            name,
            driver,
        });
    }
    gpus.sort_by_key(|g| (!g.boot_vga, g.slot.clone()));
    gpus
}

/// The GPUs of this machine.
pub fn list() -> Vec<Gpu> {
    list_in(Path::new("/sys"), pci_ids().as_deref())
}

/// The GPU `auto` picks: a dedicated one (NVIDIA first, then one that is not the boot
/// GPU), else the boot GPU.
pub fn pick_auto(gpus: &[Gpu]) -> Option<&Gpu> {
    gpus.iter().min_by_key(|g| (g.integrated, g.vendor != Vendor::Nvidia, g.boot_vga, g.slot.clone()))
}

/// What `[gpu] device` resolves to on this list of GPUs.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice<'a> {
    /// Leave it to the system (`system`, or a machine with no GPU found).
    System,
    Gpu(&'a Gpu),
    /// A PCI address that is not on this machine (now): treated as `system`.
    Missing(String),
}

pub fn choose<'a>(device: &str, gpus: &'a [Gpu]) -> Choice<'a> {
    match device.trim() {
        "" | DEVICE_AUTO => pick_auto(gpus).map_or(Choice::System, Choice::Gpu),
        DEVICE_SYSTEM => Choice::System,
        slot => gpus.iter().find(|g| g.slot.eq_ignore_ascii_case(slot)).map_or(Choice::Missing(slot.to_owned()), Choice::Gpu),
    }
}

/// The variables that send Vulkan, OpenGL and Wine to `gpu` on a machine with `gpus`.
/// Nothing on a machine with one GPU.
pub fn select_env(gpu: &Gpu, gpus: &[Gpu]) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if gpus.len() < 2 {
        return env;
    }
    let set = |env: &mut BTreeMap<String, String>, k: &str, v: String| {
        env.insert(k.to_owned(), v);
    };
    set(&mut env, "MESA_VK_DEVICE_SELECT", format!("{}!", gpu.ids()));
    if gpu.nvidia_blob() {
        set(&mut env, "__NV_PRIME_RENDER_OFFLOAD", "1".into());
        set(&mut env, "__GLX_VENDOR_LIBRARY_NAME", "nvidia".into());
        set(&mut env, "__VK_LAYER_NV_optimus", "NVIDIA_only".into());
    } else {
        if !gpu.boot_vga {
            set(&mut env, "DRI_PRIME", gpu.dri_prime_tag());
        }
        if gpus.iter().any(Gpu::nvidia_blob) {
            set(&mut env, "__VK_LAYER_NV_optimus", "non_NVIDIA_only".into());
        }
    }
    env
}

/// A vendor's preset: variables its driver (or Proton) documents that help a large
/// D3D12 title through vkd3d-proton. Kept short on purpose: each one is safe on every
/// driver version that knows it, and ignored by the others.
pub fn preset(vendor: Vendor) -> BTreeMap<String, String> {
    let pairs: &[(&str, &str)] = match vendor {
        // NVAPI (DLSS, Reflex) through Proton's dxvk-nvapi; the GL/Vulkan shader cache is
        // kept whole instead of trimmed to its default 1 GiB.
        Vendor::Nvidia => &[
            ("PROTON_ENABLE_NVAPI", "1"),
            ("DXVK_ENABLE_NVAPI", "1"),
            ("__GL_SHADER_DISK_CACHE", "1"),
            ("__GL_SHADER_DISK_CACHE_SIZE", "10737418240"),
            ("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1"),
        ],
        // RADV over AMDVLK when both are installed (vkd3d-proton is developed on RADV), and
        // room in Mesa's shader cache for a whole title's pipelines.
        Vendor::Amd => &[("AMD_VULKAN_ICD", "RADV"), ("MESA_SHADER_CACHE_MAX_SIZE", "10G")],
        Vendor::Intel => &[("MESA_SHADER_CACHE_MAX_SIZE", "10G")],
        Vendor::Other => &[],
    };
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Everything `[gpu]` adds to a run, on a machine with `gpus`.
pub fn env_for(cfg: &Config, gpus: &[Gpu]) -> BTreeMap<String, String> {
    let choice = choose(&cfg.gpu.device, gpus);
    let mut env = match &choice {
        Choice::Gpu(g) => select_env(g, gpus),
        _ => BTreeMap::new(),
    };
    let vendor = match cfg.gpu.profile {
        GpuProfile::Off => None,
        GpuProfile::Nvidia => Some(Vendor::Nvidia),
        GpuProfile::Amd => Some(Vendor::Amd),
        GpuProfile::Intel => Some(Vendor::Intel),
        GpuProfile::Auto => match &choice {
            Choice::Gpu(g) => Some(g.vendor),
            // The system's choice: the boot GPU, unless something else is set up.
            _ => gpus.first().map(|g| g.vendor),
        },
    };
    if let Some(v) = vendor {
        env.extend(preset(v));
    }
    env
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::GpuProfile;

    /// A fake `/sys/class/drm/cardN` with its PCI device.
    pub fn fake_card(sys: &Path, slot: &str, vendor: u16, device: u16, driver: &str, boot: bool, vram: Option<u64>) {
        let dev = sys.join("bus/pci/devices").join(slot);
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("vendor"), format!("0x{vendor:04x}\n")).unwrap();
        fs::write(dev.join("device"), format!("0x{device:04x}\n")).unwrap();
        fs::write(dev.join("uevent"), format!("DRIVER={driver}\nPCI_SLOT_NAME={slot}\n")).unwrap();
        fs::write(dev.join("boot_vga"), if boot { "1\n" } else { "0\n" }).unwrap();
        if let Some(v) = vram {
            fs::write(dev.join("mem_info_vram_total"), format!("{v}\n")).unwrap();
        }
        let drv = sys.join("bus/pci/drivers").join(driver);
        fs::create_dir_all(&drv).unwrap();
        std::os::unix::fs::symlink(&drv, dev.join("driver")).unwrap();
        let n = fs::read_dir(sys.join("class/drm")).map_or(0, |rd| rd.count() / 2);
        let card = sys.join("class/drm").join(format!("card{n}"));
        fs::create_dir_all(&card).unwrap();
        std::os::unix::fs::symlink(&dev, card.join("device")).unwrap();
        // A connector: not a GPU.
        let conn = sys.join("class/drm").join(format!("card{n}-eDP-1"));
        fs::create_dir_all(&conn).unwrap();
    }

    const IDS: &str = "\
# comment
8086  Intel Corporation
\ta780  Raptor Lake-S GT1 [UHD Graphics 770]
\t\t1043 8882  something
10de  NVIDIA Corporation
\t25a2  GA107M [GeForce RTX 3050 Mobile]
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t73df  Navi 22 [Radeon RX 6700/6700 XT/6750 XT / 6800M/6850M XT]
";

    /// Isaac's laptop: Intel UHD 770 on the screen, RTX 3050 for games.
    pub fn hybrid_nvidia(sys: &Path) -> Vec<Gpu> {
        fake_card(sys, "0000:01:00.0", VENDOR_NVIDIA, 0x25a2, "nvidia", false, None);
        fake_card(sys, "0000:00:02.0", VENDOR_INTEL, 0xa780, "i915", true, None);
        list_in(sys, Some(IDS))
    }

    #[test]
    fn reads_sysfs_and_pci_ids() {
        let t = tempfile::tempdir().unwrap();
        let gpus = hybrid_nvidia(t.path());
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].slot, "0000:00:02.0");
        assert_eq!(gpus[0].name, "Intel UHD Graphics 770");
        assert!(gpus[0].boot_vga && gpus[0].integrated);
        assert_eq!(gpus[1].name, "NVIDIA GeForce RTX 3050 Mobile");
        assert_eq!((gpus[1].driver.as_str(), gpus[1].ids()), ("nvidia", "10de:25a2".to_string()));
        assert!(!gpus[1].integrated);
        assert_eq!(list_in(t.path(), None)[1].name, "NVIDIA 10de:25a2");
    }

    #[test]
    fn pci_names() {
        assert_eq!(pci_name(IDS, 0x1002, 0x73df).as_deref(), Some("Radeon RX 6700/6700 XT/6750 XT / 6800M/6850M XT"));
        assert_eq!(pci_name(IDS, 0x10de, 0x1234), None);
        assert_eq!(pci_name(IDS, 0x1043, 0x8882), None);
    }

    #[test]
    fn auto_prefers_the_dedicated_gpu() {
        let t = tempfile::tempdir().unwrap();
        let gpus = hybrid_nvidia(t.path());
        assert_eq!(choose("auto", &gpus), Choice::Gpu(&gpus[1]));
        assert_eq!(choose("", &gpus), Choice::Gpu(&gpus[1]));
        assert_eq!(choose("system", &gpus), Choice::System);
        assert_eq!(choose("0000:00:02.0", &gpus), Choice::Gpu(&gpus[0]));
        assert_eq!(choose("0000:09:00.0", &gpus), Choice::Missing("0000:09:00.0".into()));
        assert_eq!(choose("auto", &[]), Choice::System);
    }

    #[test]
    fn desktop_with_igpu_still_picks_the_card() {
        // The card drives the monitor (boot VGA); the CPU's GPU is enabled too.
        let t = tempfile::tempdir().unwrap();
        fake_card(t.path(), "0000:03:00.0", VENDOR_AMD, 0x73df, "amdgpu", true, Some(12 << 30));
        fake_card(t.path(), "0000:00:02.0", VENDOR_INTEL, 0xa780, "i915", false, None);
        let gpus = list_in(t.path(), Some(IDS));
        assert_eq!(choose("auto", &gpus), Choice::Gpu(&gpus[0]));
        let env = select_env(&gpus[0], &gpus);
        assert_eq!(env.get("MESA_VK_DEVICE_SELECT").map(String::as_str), Some("1002:73df!"));
        assert!(!env.contains_key("DRI_PRIME"), "the boot GPU needs no offload: {env:?}");
    }

    #[test]
    fn amd_apu_with_amd_dgpu() {
        let t = tempfile::tempdir().unwrap();
        fake_card(t.path(), "0000:05:00.0", VENDOR_AMD, 0x1681, "amdgpu", true, Some(512 << 20));
        fake_card(t.path(), "0000:03:00.0", VENDOR_AMD, 0x73df, "amdgpu", false, Some(12 << 30));
        let gpus = list_in(t.path(), None);
        let Choice::Gpu(g) = choose("auto", &gpus) else { panic!() };
        assert_eq!(g.slot, "0000:03:00.0");
        let env = select_env(g, &gpus);
        assert_eq!(env.get("DRI_PRIME").map(String::as_str), Some("pci-0000_03_00_0"));
        assert!(!env.contains_key("__VK_LAYER_NV_optimus"));
    }

    #[test]
    fn nvidia_offload_and_its_opposite() {
        let t = tempfile::tempdir().unwrap();
        let gpus = hybrid_nvidia(t.path());
        let env = select_env(&gpus[1], &gpus);
        assert_eq!(env["__NV_PRIME_RENDER_OFFLOAD"], "1");
        assert_eq!(env["__VK_LAYER_NV_optimus"], "NVIDIA_only");
        assert_eq!(env["__GLX_VENDOR_LIBRARY_NAME"], "nvidia");
        assert_eq!(env["MESA_VK_DEVICE_SELECT"], "10de:25a2!");
        // The iGPU on purpose (battery): the NVIDIA GPU is hidden.
        let env = select_env(&gpus[0], &gpus);
        assert_eq!(env["__VK_LAYER_NV_optimus"], "non_NVIDIA_only");
        assert_eq!(env["MESA_VK_DEVICE_SELECT"], "8086:a780!");
        assert!(!env.contains_key("__NV_PRIME_RENDER_OFFLOAD"));
        // One GPU: nothing to steer.
        assert!(select_env(&gpus[1], &gpus[1..]).is_empty());
    }

    #[test]
    fn presets_follow_the_chosen_gpu() {
        let t = tempfile::tempdir().unwrap();
        let gpus = hybrid_nvidia(t.path());
        let mut cfg = Config::default();
        let env = env_for(&cfg, &gpus);
        assert_eq!(env["PROTON_ENABLE_NVAPI"], "1");
        assert_eq!(env["__NV_PRIME_RENDER_OFFLOAD"], "1");
        cfg.gpu.device = "0000:00:02.0".into();
        let env = env_for(&cfg, &gpus);
        assert_eq!(env["MESA_SHADER_CACHE_MAX_SIZE"], "10G");
        assert!(!env.contains_key("PROTON_ENABLE_NVAPI"));
        cfg.gpu.profile = GpuProfile::Off;
        assert!(!env_for(&cfg, &gpus).contains_key("MESA_SHADER_CACHE_MAX_SIZE"));
        cfg.gpu.profile = GpuProfile::Amd;
        assert_eq!(env_for(&cfg, &gpus)["AMD_VULKAN_ICD"], "RADV");
        cfg.gpu.device = "system".into();
        cfg.gpu.profile = GpuProfile::Auto;
        let env = env_for(&cfg, &gpus);
        assert!(!env.contains_key("MESA_VK_DEVICE_SELECT"));
        assert_eq!(env["MESA_SHADER_CACHE_MAX_SIZE"], "10G"); // the boot GPU is the Intel one
    }
}
