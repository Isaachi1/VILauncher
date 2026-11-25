//! One-click fixes for machine checks that need root: kernel settings (sysctl).
//!
//! A fix writes the setting to its own file in `/etc/sysctl.d` (so it survives a reboot)
//! and applies it right away with `sysctl -w`. Both need root, so they run as one
//! `pkexec /bin/sh -c ...`: the desktop asks for the administrator password once.
//! In a Flatpak, pkexec runs on the host through `flatpak-spawn --host` ([`host`]);
//! the kernel settings are the host's anyway.
//!
//! Nothing is changed without the user asking: the window shows what a fix writes, and
//! the commands to do it by hand ([`Fix::manual`]), before it runs.

use std::ffi::OsString;

use serde::Serialize;

use crate::doctor::MAX_MAP_COUNT;
use crate::host;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fix {
    /// `vm.max_map_count`: large titles map more regions than the default 65530.
    MaxMapCount,
    /// `vm.unprivileged_userfaultfd`: Wine backs vkd3d-proton's write watches with it.
    Userfaultfd,
}

pub const ALL: [Fix; 2] = [Fix::MaxMapCount, Fix::Userfaultfd];

/// What a fix does, for the window: the setting, its new value, the file it is kept in,
/// and the same thing as terminal commands.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Plan {
    pub id: Fix,
    pub setting: &'static str,
    pub value: String,
    pub file: String,
    pub manual: String,
}

impl Fix {
    pub fn parse(id: &str) -> Option<Fix> {
        match id.replace('-', "_").as_str() {
            "max_map_count" | "vm.max_map_count" => Some(Fix::MaxMapCount),
            "userfaultfd" | "vm.unprivileged_userfaultfd" => Some(Fix::Userfaultfd),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Fix::MaxMapCount => "max_map_count",
            Fix::Userfaultfd => "userfaultfd",
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            Fix::MaxMapCount => "vm.max_map_count",
            Fix::Userfaultfd => "vm.unprivileged_userfaultfd",
        }
    }

    pub fn value(self) -> String {
        match self {
            Fix::MaxMapCount => MAX_MAP_COUNT.to_string(),
            Fix::Userfaultfd => "1".into(),
        }
    }

    /// Where the setting is kept. `80-` sorts after the distributions' own files
    /// (SteamOS, Fedora and Arch ship max_map_count in 10-/50- files), so ours wins.
    pub fn file(self) -> String {
        format!("/etc/sysctl.d/80-sixlauncher-{}.conf", self.id().replace('_', "-"))
    }

    pub fn file_text(self) -> String {
        format!("# Written by SixLauncher (Check tab). Delete this file and reboot to undo it.\n{} = {}\n", self.setting(), self.value())
    }

    /// The commands this fix runs as root.
    pub fn script(self) -> String {
        format!(
            "mkdir -p /etc/sysctl.d && printf '%s' '{}' > {} && sysctl -w {}={}",
            self.file_text(),
            self.file(),
            self.setting(),
            self.value()
        )
    }

    /// The same, to paste in a terminal.
    pub fn manual(self) -> String {
        format!(
            "echo '{} = {}' | sudo tee {}\nsudo sysctl -w {}={}",
            self.setting(),
            self.value(),
            self.file(),
            self.setting(),
            self.value()
        )
    }

    pub fn plan(self) -> Plan {
        Plan { id: self, setting: self.setting(), value: self.value(), file: self.file(), manual: self.manual() }
    }
}

/// The command that applies `fixes` with one password prompt.
pub fn argv(fixes: &[Fix]) -> Vec<OsString> {
    let script = fixes.iter().map(|f| f.script()).collect::<Vec<_>>().join(" && ");
    vec!["pkexec".into(), "/bin/sh".into(), "-c".into(), script.into()]
}

/// What pkexec's exit code means, for the user.
pub fn explain_exit(code: Option<i32>, fixes: &[Fix]) -> Option<String> {
    let manual = fixes.iter().map(|f| f.manual()).collect::<Vec<_>>().join("\n");
    match code {
        Some(0) => None,
        Some(126) => Some("cancelled: the password prompt was closed; nothing was changed".into()),
        Some(127) => Some(format!(
            "not authorized: the password was refused, or no polkit agent is running to ask for it. \
             In a terminal it is:\n{manual}"
        )),
        Some(c) => Some(format!("the fix failed (exit code {c}). In a terminal it is:\n{manual}")),
        None => Some(format!("the fix was interrupted. In a terminal it is:\n{manual}")),
    }
}

fn pkexec_available() -> bool {
    host::simple(&["sh".into(), "-c".into(), "command -v pkexec".into()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Applies `fixes` as root (pkexec asks for the password). The caller checks again afterwards.
pub fn apply(fixes: &[Fix]) -> Result<()> {
    if fixes.is_empty() {
        return Ok(());
    }
    let manual = fixes.iter().map(|f| f.manual()).collect::<Vec<_>>().join("\n");
    if !pkexec_available() {
        return Err(Error::Config(format!("pkexec (polkit) is not installed, so the window cannot ask for the password. In a terminal it is:\n{manual}")));
    }
    let status = host::simple(&argv(fixes)).status().map_err(|e| Error::Config(format!("pkexec: {e}. In a terminal it is:\n{manual}")))?;
    match explain_exit(status.code(), fixes) {
        None => Ok(()),
        Some(why) => Err(Error::Config(why)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for f in ALL {
            assert_eq!(Fix::parse(f.id()), Some(f));
            assert_eq!(Fix::parse(f.setting()), Some(f));
        }
        assert_eq!(Fix::parse("max-map-count"), Some(Fix::MaxMapCount));
        assert_eq!(Fix::parse("swap"), None);
        assert_eq!(serde_json::to_string(&Fix::MaxMapCount).unwrap(), "\"max_map_count\"");
    }

    #[test]
    fn one_password_prompt_for_every_fix() {
        let argv = argv(&ALL);
        assert_eq!(&argv[..3], ["pkexec", "/bin/sh", "-c"].map(OsString::from));
        let script = argv[3].to_string_lossy();
        assert!(script.contains("> /etc/sysctl.d/80-sixlauncher-max-map-count.conf && sysctl -w vm.max_map_count=1048576"), "{script}");
        assert!(script.contains("> /etc/sysctl.d/80-sixlauncher-userfaultfd.conf && sysctl -w vm.unprivileged_userfaultfd=1"), "{script}");
        assert!(!Fix::Userfaultfd.file_text().contains('\''));
    }

    #[test]
    fn the_script_writes_the_file() {
        // The script, with /etc swapped for a temporary folder and sysctl for true.
        let t = tempfile::tempdir().unwrap();
        let etc = t.path().to_string_lossy().into_owned();
        let script = Fix::MaxMapCount.script().replace("/etc", &etc).replace("sysctl -w", "true");
        assert!(std::process::Command::new("/bin/sh").args(["-c", &script]).status().unwrap().success());
        let text = std::fs::read_to_string(format!("{etc}/sysctl.d/80-sixlauncher-max-map-count.conf")).unwrap();
        assert!(text.ends_with("\nvm.max_map_count = 1048576\n"), "{text}");
    }

    #[test]
    fn pkexec_exit_codes() {
        assert_eq!(explain_exit(Some(0), &ALL), None);
        assert!(explain_exit(Some(126), &ALL).unwrap().starts_with("cancelled"));
        let refused = explain_exit(Some(127), &[Fix::Userfaultfd]).unwrap();
        assert!(refused.contains("sudo sysctl -w vm.unprivileged_userfaultfd=1"), "{refused}");
    }
}
