//! Release-version check for `doctor` and stale-binary detection.
//!
//! Only `doctor` (CLI and MCP) contacts GitHub, at most once per 24h; the
//! result is cached in `~/.rms-memory/update-check.json`. Server start-up and
//! MCP `initialize` read the cache only and never touch the network. Set
//! `RMS_MEMORY_NO_UPDATE_CHECK=1` to disable the check (also skipped under CI).
//! Upgrading is left to the package manager that installed the binary.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/max-ramas/rms-memory-mcp/releases/latest";
const CHECK_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheck {
    pub latest: String,
    pub release_url: String,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

impl UpdateCheck {
    pub fn update_available(&self) -> bool {
        is_newer(&self.latest, CURRENT_VERSION)
    }
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(current.trim_start_matches('v')),
    ) {
        (Ok(candidate), Ok(current)) => candidate > current,
        _ => false,
    }
}

pub fn checks_disabled() -> bool {
    let set =
        |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty() && v != "0" && v != "false");
    set("RMS_MEMORY_NO_UPDATE_CHECK") || set("CI")
}

/// Upgrade command matching how the running binary was installed.
pub fn upgrade_hint(exe: &Path) -> &'static str {
    let path = exe.to_string_lossy().replace('\\', "/");
    if path.contains("/Cellar/") || path.contains("/homebrew/") || path.contains("/linuxbrew/") {
        "brew upgrade rms-memory-mcp"
    } else if path.contains("/.cargo/bin/") {
        "cargo install --locked rms-memory-mcp"
    } else if path.starts_with("/usr/bin/") {
        "your package manager (.deb/.rpm from the release page)"
    } else {
        "the release archive or `scripts/install.sh`"
    }
}

fn cache_path() -> PathBuf {
    crate::workspace::base_dir().join("update-check.json")
}

/// Last cached result; never touches the network.
pub fn cached_check() -> Option<UpdateCheck> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

fn fetch_latest() -> Result<UpdateCheck> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .user_agent(format!("rms-memory/{CURRENT_VERSION}"))
        .build()
        .into();
    let release: LatestRelease = agent
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("GitHub release lookup failed")?
        .body_mut()
        .read_json()
        .context("Unexpected GitHub release response")?;
    Ok(UpdateCheck {
        latest: release.tag_name.trim_start_matches('v').to_string(),
        release_url: release.html_url,
        checked_at: chrono::Utc::now(),
    })
}

/// Doctor-only: returns the cache when younger than 24h, otherwise asks
/// GitHub once and refreshes the cache. `None` when disabled or offline.
pub fn check_for_doctor() -> Option<UpdateCheck> {
    if checks_disabled() {
        return None;
    }
    if let Some(cached) = cached_check()
        && (chrono::Utc::now() - cached.checked_at)
            .to_std()
            .is_ok_and(|age| age < CHECK_TTL)
    {
        return Some(cached);
    }
    match fetch_latest() {
        Ok(check) => {
            let path = cache_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if let Ok(json) = serde_json::to_string_pretty(&check) {
                std::fs::write(&path, json).ok();
            }
            Some(check)
        }
        Err(error) => {
            tracing::debug!("Update check skipped: {error:#}");
            cached_check()
        }
    }
}

fn notice_for(check: &UpdateCheck) -> Option<String> {
    check.update_available().then(|| {
        let hint = std::env::current_exe()
            .map(|exe| upgrade_hint(&std::fs::canonicalize(&exe).unwrap_or(exe)))
            .unwrap_or("your package manager");
        format!(
            "rms-memory {} is available (running {CURRENT_VERSION}). Upgrade with {hint}, then restart MCP clients. Release notes: {}",
            check.latest, check.release_url
        )
    })
}

/// Notice from the cache only, for MCP `initialize`.
pub fn cached_update_notice() -> Option<String> {
    if checks_disabled() {
        return None;
    }
    notice_for(&cached_check()?)
}

/// Size and mtime of the executable this process was started from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryFingerprint {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl BinaryFingerprint {
    pub fn capture() -> Option<Self> {
        let path = std::env::current_exe().ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        Some(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            path,
        })
    }

    /// Notice when the binary on disk was replaced after this process started
    /// (e.g. by an upgrade), so the running server is outdated.
    pub fn stale_notice(&self) -> Option<String> {
        let meta = std::fs::metadata(&self.path).ok()?;
        if meta.len() == self.len && meta.modified().ok() == self.modified {
            return None;
        }
        Some(format!(
            "This MCP server is still running rms-memory {CURRENT_VERSION}, but {} was replaced on disk after it started. Restart the MCP server (or the IDE) to load the new binary.",
            self.path.display()
        ))
    }
}

static STARTUP_FINGERPRINT: std::sync::OnceLock<Option<BinaryFingerprint>> =
    std::sync::OnceLock::new();

/// Records the executable fingerprint; call once at server start.
pub fn record_startup_fingerprint() {
    STARTUP_FINGERPRINT.get_or_init(BinaryFingerprint::capture);
}

/// Doctor notices: replaced server binary and a newer release, if any.
pub fn doctor_notices() -> Vec<String> {
    let mut notices = Vec::new();
    if let Some(Some(fingerprint)) = STARTUP_FINGERPRINT.get()
        && let Some(stale) = fingerprint.stale_notice()
    {
        notices.push(stale);
    }
    if let Some(check) = check_for_doctor() {
        notices.extend(notice_for(&check));
    }
    notices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_uses_semver() {
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(is_newer("v1.3.0", "1.2.2"));
        assert!(!is_newer("1.2.2", "1.2.2"));
        assert!(!is_newer("1.2.2-rc.1", "1.2.2"));
        assert!(!is_newer("garbage", "1.2.2"));
    }

    #[test]
    fn upgrade_hint_follows_install_location() {
        assert_eq!(
            upgrade_hint(Path::new(
                "/opt/homebrew/Cellar/rms-memory-mcp/1.3.0/bin/rms-memory"
            )),
            "brew upgrade rms-memory-mcp"
        );
        assert_eq!(
            upgrade_hint(Path::new("/Users/x/.cargo/bin/rms-memory")),
            "cargo install --locked rms-memory-mcp"
        );
        assert!(upgrade_hint(Path::new("/usr/bin/rms-memory")).contains("package manager"));
    }

    #[test]
    fn stale_fingerprint_detects_replaced_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        std::fs::write(&path, b"old").unwrap();
        let fingerprint = BinaryFingerprint {
            path: path.clone(),
            len: 3,
            modified: std::fs::metadata(&path).unwrap().modified().ok(),
        };
        assert!(fingerprint.stale_notice().is_none());
        std::fs::write(&path, b"replaced binary").unwrap();
        assert!(fingerprint.stale_notice().is_some());
    }
}
