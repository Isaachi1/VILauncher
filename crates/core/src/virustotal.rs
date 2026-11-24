//! Looks up backend builds on VirusTotal before they run.
//!
//! With an API key (a free VirusTotal account has one), every `.exe` and `.dll`
//! of a runtime the launcher is about to install, or of the backend folder in
//! use, is looked up by its SHA-256. Only the hash leaves the machine. A file
//! VirusTotal has never seen can be uploaded on request, which shares it with
//! VirusTotal and its partners; that is offered only for the backend's own
//! files, never for anything in the game folder.
//!
//! The key comes from `VILAUNCHER_VT_KEY` or `~/.config/vilauncher/virustotal.key`
//! (written with mode 0600), never from `config.toml`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::update::sha256_file;
use crate::{Error, Result, net, paths};

const API: &str = "https://www.virustotal.com/api/v3";

/// `VILAUNCHER_VT_API` points the client at another server (a local stand-in, for tests).
fn api() -> String {
    std::env::var("VILAUNCHER_VT_API").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| API.into())
}
/// Files up to this size go to `/files`; larger ones need an upload URL first.
const DIRECT_UPLOAD: u64 = 32 << 20;
/// VirusTotal's own limit.
const MAX_UPLOAD: u64 = 650 << 20;

pub fn key_file() -> PathBuf {
    paths::config_dir().join("virustotal.key")
}

pub fn key() -> Option<String> {
    let from_env = std::env::var("VILAUNCHER_VT_KEY").ok();
    let k = from_env.or_else(|| fs::read_to_string(key_file()).ok())?;
    let k = k.trim().to_owned();
    (!k.is_empty()).then_some(k)
}

/// Saves the key readable only by the user; an empty key removes it.
pub fn save_key(key: &str) -> Result<()> {
    let path = key_file();
    let key = key.trim();
    if key.is_empty() {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        return Ok(());
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(Error::Config("a VirusTotal API key has only letters and digits".into()));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let _ = fs::remove_file(&path);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
    writeln!(f, "{key}")?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Known to VirusTotal and no engine calls it malicious.
    Clean,
    /// At least one engine calls it malicious.
    Flagged,
    /// VirusTotal has not seen this file (or has not finished with it).
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub file: String,
    pub sha256: String,
    pub state: State,
    pub malicious: u32,
    pub suspicious: u32,
    /// Engines that gave an answer.
    pub engines: u32,
    pub link: String,
}

fn link(sha256: &str) -> String {
    format!("https://www.virustotal.com/gui/file/{sha256}")
}

/// Reads a `GET /files/{hash}` answer (None: 404, never seen).
pub fn verdict(file: &str, sha256: &str, report: Option<&str>) -> Result<Verdict> {
    let mut v = Verdict { file: file.into(), sha256: sha256.into(), state: State::Unknown, malicious: 0, suspicious: 0, engines: 0, link: link(sha256) };
    let Some(report) = report else { return Ok(v) };
    let json: serde_json::Value = serde_json::from_str(report).map_err(|e| Error::Config(format!("VirusTotal: {e}")))?;
    let stats = &json["data"]["attributes"]["last_analysis_stats"];
    let n = |k: &str| stats[k].as_u64().unwrap_or(0) as u32;
    v.malicious = n("malicious");
    v.suspicious = n("suspicious");
    v.engines = ["malicious", "suspicious", "harmless", "undetected"].iter().map(|k| n(k)).sum();
    v.state = if v.malicious > 0 {
        State::Flagged
    } else if v.engines == 0 {
        State::Unknown
    } else {
        State::Clean
    };
    Ok(v)
}

/// The `.exe` and `.dll` files directly in `dir`, by name.
pub fn binaries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("dll")))
        .collect();
    out.sort();
    Ok(out)
}

/// Looks up every binary of `dir`; `lookup` turns a SHA-256 into the API's answer.
pub fn check_dir_with(dir: &Path, lookup: &mut dyn FnMut(&str) -> Result<Option<String>>) -> Result<Vec<Verdict>> {
    let mut out = Vec::new();
    for path in binaries(dir)? {
        let sha = sha256_file(&path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(verdict(&name, &sha, lookup(&sha)?.as_deref())?);
    }
    Ok(out)
}

/// Why `verdicts` should stop a run or an install, if they should.
pub fn blocking(verdicts: &[Verdict]) -> Option<String> {
    let flagged: Vec<String> = verdicts
        .iter()
        .filter(|v| v.state == State::Flagged)
        .map(|v| format!("{}: {} of {} engines call it malicious ({})", v.file, v.malicious, v.engines, v.link))
        .collect();
    (!flagged.is_empty()).then(|| format!("VirusTotal flags this build:\n{}", flagged.join("\n")))
}

fn api_error(url: &str, e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(401, _) => Error::Config("VirusTotal refused the API key (401); check it in Compatibilidade".into()),
        ureq::Error::Status(429, _) => Error::Config("VirusTotal's request limit was reached (a free key allows 4 a minute); try again in a minute".into()),
        e => net::error(url, e),
    }
}

/// `GET /files/{sha256}`: the report, or None when VirusTotal has never seen the file.
pub fn lookup(key: &str, sha256: &str) -> Result<Option<String>> {
    let url = format!("{}/files/{sha256}", api());
    match net::agent(Duration::from_secs(30)).get(&url).set("x-apikey", key).call() {
        Ok(resp) => resp.into_string().map(Some).map_err(|e| Error::Config(format!("VirusTotal: {e}"))),
        Err(ureq::Error::Status(404, _)) => Ok(None),
        Err(e) => Err(api_error(&url, e)),
    }
}

pub fn check_dir(dir: &Path, key: &str) -> Result<Vec<Verdict>> {
    check_dir_with(dir, &mut |sha| lookup(key, sha))
}

fn multipart(name: &str, data: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("vilauncher-{}", crate::update::sha256_hex(data).get(..24).unwrap_or("boundary"));
    let safe: String = name.chars().filter(|c| c.is_ascii_graphic() && *c != '"' && *c != '\\').collect();
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Uploads `path` for analysis and returns the analysis id. The file becomes
/// available to VirusTotal and its partners.
pub fn upload(key: &str, path: &Path) -> Result<String> {
    let size = fs::metadata(path)?.len();
    if size > MAX_UPLOAD {
        return Err(Error::Config(format!("{}: larger than VirusTotal accepts", path.display())));
    }
    let agent = net::agent(Duration::from_secs(600));
    let url = if size > DIRECT_UPLOAD {
        let u = format!("{}/files/upload_url", api());
        let resp = agent.get(&u).set("x-apikey", key).call().map_err(|e| api_error(&u, e))?;
        let json: serde_json::Value = serde_json::from_reader(resp.into_reader()).map_err(|e| Error::Config(format!("VirusTotal: {e}")))?;
        json["data"].as_str().ok_or_else(|| Error::Config("VirusTotal: no upload URL".into()))?.to_owned()
    } else {
        format!("{}/files", api())
    };
    let data = fs::read(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let (content_type, body) = multipart(&name, &data);
    let resp = agent.post(&url).set("x-apikey", key).set("Content-Type", &content_type).send_bytes(&body).map_err(|e| api_error(&url, e))?;
    let json: serde_json::Value = serde_json::from_reader(resp.into_reader()).map_err(|e| Error::Config(format!("VirusTotal: {e}")))?;
    json["data"]["id"].as_str().map(str::to_owned).ok_or_else(|| Error::Config("VirusTotal: no analysis id in the answer".into()))
}

/// The backend folder to check: the backend executable's folder, refused when it
/// is (or is inside) the game folder, so no game file is ever looked up or sent.
pub fn backend_dir(cfg: &crate::config::Config) -> Result<PathBuf> {
    let exe = cfg.backend_exe().ok_or_else(|| Error::Config("no backend executable is set".into()))?;
    let dir = paths::normalize(exe.parent().unwrap_or(Path::new("/")));
    if let Some(game) = cfg.game_dir() {
        let game = paths::normalize(&game);
        if dir.starts_with(&game) || game.starts_with(&dir) {
            return Err(Error::Config("the backend's folder holds the game folder or is inside it; VirusTotal checks only the backend's own folder".into()));
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testutil::{self, TempHome};

    fn report(mal: u32, harmless: u32) -> String {
        format!(r#"{{"data": {{"attributes": {{"last_analysis_stats": {{"malicious": {mal}, "suspicious": 1, "harmless": {harmless}, "undetected": 60}}}}}}}}"#)
    }

    #[test]
    fn verdicts_from_reports() {
        let clean = verdict("a.exe", "ab", Some(&report(0, 5))).unwrap();
        assert_eq!((clean.state, clean.engines, clean.suspicious), (State::Clean, 66, 1));
        let bad = verdict("a.exe", "ab", Some(&report(3, 5))).unwrap();
        assert_eq!((bad.state, bad.malicious), (State::Flagged, 3));
        assert_eq!(verdict("a.exe", "ab", None).unwrap().state, State::Unknown);
        let queued = verdict("a.exe", "ab", Some(r#"{"data": {"attributes": {}}}"#)).unwrap();
        assert_eq!(queued.state, State::Unknown);
        assert_eq!(bad.link, "https://www.virustotal.com/gui/file/ab");
    }

    #[test]
    fn checks_only_binaries_and_blocks_on_malicious() {
        let t = tempfile::tempdir().unwrap();
        let exe = testutil::fake_pe(&t.path().join("xcl.exe"), 0x8664);
        fs::write(t.path().join("opus.DLL"), "MZ another file").unwrap();
        fs::write(t.path().join("XCL_COMMIT.txt"), "xcl abc\n").unwrap();
        fs::create_dir_all(t.path().join("licenses")).unwrap();
        let exe_sha = sha256_file(&exe).unwrap();

        let mut asked = Vec::new();
        let v = check_dir_with(t.path(), &mut |sha| {
            asked.push(sha.to_owned());
            Ok((sha == exe_sha).then(|| report(2, 5)))
        })
        .unwrap();
        assert_eq!(v.iter().map(|v| v.file.as_str()).collect::<Vec<_>>(), ["opus.DLL", "xcl.exe"]);
        assert_eq!(asked.len(), 2);
        assert_eq!((v[0].state, v[1].state), (State::Unknown, State::Flagged));
        let why = blocking(&v).unwrap();
        assert!(why.contains("xcl.exe: 2 of 68"), "{why}");
        assert_eq!(blocking(&v[..1]), None);
    }

    #[test]
    fn key_is_private_and_can_be_removed() {
        let h = TempHome::new();
        assert_eq!(key(), None);
        save_key(" abc123 \n").unwrap();
        assert_eq!(key().as_deref(), Some("abc123"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(key_file()).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(save_key("abc; rm").is_err());
        save_key("").unwrap();
        assert_eq!(key(), None);
        assert!(key_file().starts_with(h.home()));
    }

    #[test]
    fn game_folder_is_never_checked() {
        let h = TempHome::new();
        let game = testutil::fake_game(h.path());
        let mut cfg = Config::default();
        cfg.game.dir = Some(game.clone());
        cfg.backend.exe = Some(game.join("bin/xcl.exe"));
        assert!(backend_dir(&cfg).is_err());
        cfg.backend.exe = Some(h.path().join("xcl.exe"));
        assert!(backend_dir(&cfg).is_err(), "a folder that holds the game is refused too");
        cfg.backend.exe = Some(h.path().join("builds/xcl/xcl.exe"));
        assert_eq!(backend_dir(&cfg).unwrap(), h.path().join("builds/xcl"));
    }

    #[test]
    fn multipart_body_wraps_the_file() {
        let (ct, body) = multipart("x\"y.exe", b"DATA");
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("filename=\"xy.exe\""));
        assert!(text.ends_with(&format!("\r\n\r\nDATA\r\n--{boundary}--\r\n")));
    }
}
