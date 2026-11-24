//! The launcher's few HTTPS requests.

use std::time::Duration;

use crate::{Error, Result};

pub const USER_AGENT: &str = concat!("VILauncher/", env!("CARGO_PKG_VERSION"));

pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(timeout).user_agent(USER_AGENT).build()
}

/// A failed request as a message for the user.
pub fn error(url: &str, e: ureq::Error) -> Error {
    match e {
        ureq::Error::Status(code, _) => Error::Config(format!("{url}: HTTP {code}")),
        ureq::Error::Transport(t) => Error::Config(t.to_string()),
    }
}

/// GET `url` as text (at most 10 MB).
pub fn get_text(url: &str, timeout: Duration) -> Result<String> {
    let resp = agent(timeout).get(url).call().map_err(|e| error(url, e))?;
    resp.into_string().map_err(|e| Error::Config(format!("{url}: {e}")))
}
