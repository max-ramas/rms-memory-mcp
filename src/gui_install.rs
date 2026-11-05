//! Installing and opening the companion GUI from the command line (`rms-memory gui`).
//!
//! The installer comes from the RMS update center through the same trust chain as
//! `rms-memory update`: a minisign-signed manifest names the file, its size and its sha256, and
//! nothing is opened before the download matches. The GUI itself stays a separate, licensed
//! product (it starts with a trial); nothing here is needed to run the MCP.

use crate::updater::{self, Candidate, UpdateOptions};
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const GUI_PRODUCT: &str = "rms-memory-gui";

/// `(platform, arch, format)` of the installer published for a host, or `None` when there is no
/// GUI build for it. Values follow the update-center vocabulary.
pub fn selector_for(os: &str, arch: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match (os, arch) {
        ("macos", "aarch64") => Some(("darwin", "arm64", "dmg")),
        ("windows", "x86_64") => Some(("windows", "amd64", "exe")),
        ("linux", "x86_64") => Some(("linux", "amd64", "appimage")),
        _ => None,
    }
}

/// Update-center options for the GUI installer of this machine (downloads are routed through the
/// server and labelled `src=cli`, so they are counted).
pub fn options_for_host() -> Option<UpdateOptions> {
    let (platform, arch, format) = selector_for(std::env::consts::OS, std::env::consts::ARCH)?;
    Some(UpdateOptions {
        product: GUI_PRODUCT.to_string(),
        platform: platform.to_string(),
        arch: arch.to_string(),
        format: format.to_string(),
        src_label: "cli".to_string(),
        via_server: true,
        ..UpdateOptions::default()
    })
}

/// The newest signed GUI installer for this machine, if there is one.
pub fn find_installer(opts: &UpdateOptions) -> Result<Option<Candidate>> {
    updater::check(opts, "0.0.0")
}

/// Human-readable list of what is published, for the "no build for your machine" message.
pub fn supported_hosts() -> &'static str {
    "macOS on Apple Silicon, Windows x64 and Linux x64"
}

/// What `install` did.
#[derive(Debug)]
pub enum Placed {
    /// The app is in place and can be opened.
    Installed(PathBuf),
    /// The installer was started (Windows): the user finishes it in its own window.
    InstallerStarted(PathBuf),
    /// Only downloaded (`--download-only`).
    Downloaded(PathBuf),
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+'))
}

fn downloads_dir() -> PathBuf {
    dirs::download_dir().unwrap_or_else(std::env::temp_dir)
}

/// Download the verified installer and put it where it belongs on this OS.
pub fn install(candidate: &Candidate, download_only: bool) -> Result<Placed> {
    let file = &candidate.artifact.file;
    if !safe_name(file) {
        bail!("signed manifest contains an unsafe file name");
    }
    if download_only {
        let dest = downloads_dir().join(file);
        updater::download_artifact_to(candidate, &dest)?;
        return Ok(Placed::Downloaded(dest));
    }
    match std::env::consts::OS {
        "macos" => {
            let dir = tempfile::tempdir()?;
            let dmg = dir.path().join(file);
            updater::download_artifact_to(candidate, &dmg)?;
            Ok(Placed::Installed(install_dmg(&dmg, dir.path())?))
        }
        "windows" => {
            let dest = downloads_dir().join(file);
            updater::download_artifact_to(candidate, &dest)?;
            Command::new(&dest)
                .spawn()
                .with_context(|| format!("could not start {}", dest.display()))?;
            Ok(Placed::InstallerStarted(dest))
        }
        _ => {
            let home = dirs::home_dir().ok_or_else(|| anyhow!("no home directory"))?;
            let dir = home.join(".local").join("bin");
            std::fs::create_dir_all(&dir)?;
            let dest = dir.join("rms-memory-gui");
            updater::download_artifact_to(candidate, &dest)?;
            make_executable(&dest)?;
            Ok(Placed::Installed(dest))
        }
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Mounts a disk image, copies the app bundle into /Applications (or ~/Applications when that
/// is not writable) and unmounts again. The download is not quarantined (no browser involved),
/// so the unsigned app opens without a Gatekeeper prompt.
pub fn install_dmg(dmg: &Path, work: &Path) -> Result<PathBuf> {
    let mount = work.join("mnt");
    std::fs::create_dir_all(&mount)?;
    let attach = Command::new("hdiutil")
        .args([
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-quiet",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(dmg)
        .output()
        .context("hdiutil is not available")?;
    if !attach.status.success() {
        bail!(
            "could not open the disk image: {}",
            String::from_utf8_lossy(&attach.stderr).trim()
        );
    }
    let result = copy_app_from(&mount);
    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet", "-force"])
        .arg(&mount)
        .status();
    result
}

fn copy_app_from(mount: &Path) -> Result<PathBuf> {
    let app = std::fs::read_dir(mount)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| anyhow!("the disk image contains no application"))?;
    let name = app
        .file_name()
        .ok_or_else(|| anyhow!("unexpected application path"))?
        .to_owned();
    // `RMS_MEMORY_GUI_APPS_DIR` redirects the install (used by tests and unusual setups).
    let mut targets = match std::env::var_os("RMS_MEMORY_GUI_APPS_DIR") {
        Some(dir) if !dir.is_empty() => vec![PathBuf::from(dir)],
        _ => {
            let mut t = vec![PathBuf::from("/Applications")];
            if let Some(home) = dirs::home_dir() {
                t.push(home.join("Applications"));
            }
            t
        }
    };
    targets.dedup();
    let mut last_error = String::new();
    for dir in targets {
        let dest = dir.join(&name);
        if dest.exists() {
            bail!("{} already exists", dest.display());
        }
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        let out = Command::new("ditto").arg(&app).arg(&dest).output()?;
        if out.status.success() {
            return Ok(dest);
        }
        last_error = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let _ = std::fs::remove_dir_all(&dest);
    }
    bail!("could not copy the application: {last_error}")
}

/// Open the installed GUI without waiting for it.
pub fn launch(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]).arg(path);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        if path.extension().is_some_and(|e| e == "desktop") {
            let mut c = Command::new("gtk-launch");
            c.arg(path.file_stem().unwrap_or_default());
            c
        } else {
            Command::new(path)
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not open {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_covers_exactly_the_published_installers() {
        assert_eq!(
            selector_for("macos", "aarch64"),
            Some(("darwin", "arm64", "dmg"))
        );
        assert_eq!(
            selector_for("windows", "x86_64"),
            Some(("windows", "amd64", "exe"))
        );
        assert_eq!(
            selector_for("linux", "x86_64"),
            Some(("linux", "amd64", "appimage"))
        );
        // No GUI build: Intel macOS, ARM Windows/Linux.
        assert_eq!(selector_for("macos", "x86_64"), None);
        assert_eq!(selector_for("linux", "aarch64"), None);
        assert_eq!(selector_for("windows", "aarch64"), None);
    }

    #[test]
    fn file_names_from_the_manifest_are_vetted() {
        assert!(safe_name("rms_memory_gui_1.4.0_aarch64.dmg"));
        assert!(!safe_name("../evil.dmg"));
        assert!(!safe_name(".hidden"));
        assert!(!safe_name("a b.dmg"));
        assert!(!safe_name(""));
    }
}
