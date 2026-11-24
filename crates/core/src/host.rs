//! Running programs on the host when VILauncher is a Flatpak.
//!
//! Inside the Flatpak sandbox the launcher sees the home folder (`--filesystem=home`)
//! but not the host's `/usr`, and Proton, Wine and `wineserver` must run on the host,
//! with its libraries and GPU drivers. So every program the launcher starts goes
//! through [`command`]: outside a Flatpak that is a plain `Command`; inside, it is
//! `flatpak-spawn --host`, which needs `--talk-name=org.freedesktop.Flatpak`.
//!
//! `flatpak-spawn` gives the host program the host's own environment, not the
//! sandbox's, so only the variables the launcher sets or changes are passed on
//! (`--env=`), and the working folder with `--directory=`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Whether this process runs inside a Flatpak sandbox.
pub fn in_flatpak() -> bool {
    Path::new("/.flatpak-info").is_file() || std::env::var_os("FLATPAK_ID").is_some_and(|v| !v.is_empty())
}

/// The program and arguments to run `argv` with `env` in `cwd`, as `flatpak-spawn`
/// needs them when `flatpak` is true. `base` is this process's own environment:
/// variables equal to it are left out.
pub fn wrap(
    argv: &[OsString],
    env: &BTreeMap<String, String>,
    base: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    flatpak: bool,
) -> (OsString, Vec<OsString>) {
    if !flatpak {
        return (argv[0].clone(), argv[1..].to_vec());
    }
    let mut args: Vec<OsString> = vec!["--host".into(), "--watch-bus".into()];
    if let Some(dir) = cwd {
        let mut d = OsString::from("--directory=");
        d.push(dir);
        args.push(d);
    }
    for (k, v) in env {
        if base.get(k) != Some(v) {
            args.push(format!("--env={k}={v}").into());
        }
    }
    args.extend(argv.iter().cloned());
    ("flatpak-spawn".into(), args)
}

/// A `Command` for `argv` with `env` (the full environment the program should see)
/// and `cwd`, run on the host when this is a Flatpak.
pub fn command(argv: &[OsString], env: &BTreeMap<String, String>, cwd: Option<&Path>) -> Command {
    let flatpak = in_flatpak();
    let base: BTreeMap<String, String> = std::env::vars().collect();
    let (program, args) = wrap(argv, env, &base, cwd, flatpak);
    let mut cmd = Command::new(program);
    cmd.args(args);
    if !flatpak {
        cmd.envs(env);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
    }
    cmd
}

/// How a menu entry starts this launcher's `name` program (`vilauncher` or
/// `vilauncher-app`): `flatpak run` inside a Flatpak, where the binary's own path
/// exists only in the sandbox; otherwise the running program's path.
pub fn self_exec(name: &str) -> Vec<String> {
    match std::env::var("FLATPAK_ID") {
        Ok(id) if in_flatpak() && !id.is_empty() => vec!["flatpak".into(), "run".into(), format!("--command={name}"), id],
        _ => vec![std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| name.into())],
    }
}

/// [`command`] with nothing added to the environment.
pub fn simple(argv: &[OsString]) -> Command {
    command(argv, &BTreeMap::new(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn outside_flatpak_nothing_changes() {
        let (p, a) = wrap(&os(&["/p/proton", "run", "x.exe"]), &BTreeMap::new(), &BTreeMap::new(), None, false);
        assert_eq!((p, a), ("/p/proton".into(), os(&["run", "x.exe"])));
    }

    #[test]
    fn inside_flatpak_runs_on_the_host_with_only_the_changed_variables() {
        let base = BTreeMap::from([("PATH".to_string(), "/app/bin:/usr/bin".to_string()), ("HOME".to_string(), "/home/eu".to_string())]);
        let mut env = base.clone();
        env.insert("WINEPREFIX".into(), "/home/eu/pfx".into());
        env.insert("HOME".into(), "/home/eu".into());
        let (p, a) = wrap(&os(&["/p/proton", "run", "x.exe"]), &env, &base, Some(Path::new("/home/eu/data")), true);
        assert_eq!(p, OsString::from("flatpak-spawn"));
        assert_eq!(
            a,
            os(&["--host", "--watch-bus", "--directory=/home/eu/data", "--env=WINEPREFIX=/home/eu/pfx", "/p/proton", "run", "x.exe"])
        );
    }
}
