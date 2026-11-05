//! Self-update from the RMS update center (`https://update.rms-ds.com`).
//!
//! Trust chain (see `rms-update-center/contracts/README.md`):
//! `latest` response (locator, untrusted) -> `release.json` + `release.json.minisig`
//! (minisign prehashed `ED`, verified against keys compiled into this binary) ->
//! artifact (`size` and `sha256` taken only from the signed manifest).
//! Nothing from the archive is executed before the signature and the checksum pass,
//! and the new binary must answer `--version` with the expected version before it
//! replaces the old one. The previous binary is kept as a backup for rollback.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://update.rms-ds.com";
pub const PRODUCT: &str = "rms-memory-mcp";
pub const DEFAULT_CHANNEL: &str = "stable";
/// Environment override of the update center URL (self-hosted mirrors, tests).
pub const BASE_URL_ENV: &str = "RMS_MEMORY_UPDATE_URL";
/// Extra trusted minisign public key (full `.pub` text or just the base64 line).
pub const EXTRA_KEY_ENV: &str = "RMS_MEMORY_UPDATE_PUBKEY";

const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;

/// Release signing keys trusted by this build. Add the next key *before* rotating
/// the CI key so that already-installed binaries can verify the first release signed
/// with it. Format: minisign `.pub` text.
const TRUSTED_KEYS: &[&str] = &[
    // rel-2026a: offline production key of the update center.
    "untrusted comment: minisign public key: A6F888ED44FA43F3\nRWTzQ/pE7Yj4phFhGzE5d1fPXTUULnZzQYpeULlnjPSr/g0qrGFVrl3m",
    // ci-memory-2026a: signs rms-memory-mcp release manifests in GitHub Actions.
    "untrusted comment: minisign public key: C1F43AE0FCB3AE0C\nRWQMrrP84Dr0wbPBA3FAoYdqHLNc2nfuBxENI19mmBn+dENmYT7ShULQ",
];

pub fn trusted_keys() -> Vec<String> {
    let mut keys: Vec<String> = TRUSTED_KEYS.iter().map(|k| (*k).to_string()).collect();
    if let Ok(extra) = std::env::var(EXTRA_KEY_ENV)
        && !extra.trim().is_empty()
    {
        keys.push(extra);
    }
    keys
}

pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// `(platform, arch, format)` in update-center vocabulary for the running host.
pub fn host_selector() -> (&'static str, &'static str, &'static str) {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let format = if platform == "windows" {
        "zip"
    } else {
        "tar.gz"
    };
    (platform, arch, format)
}

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub base_url: String,
    pub product: String,
    pub channel: String,
    pub platform: String,
    pub arch: String,
    pub format: String,
    pub trusted_keys: Vec<String>,
    /// Per-request budget for the lookup (the download uses a longer fixed one).
    pub timeout: Duration,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        let (platform, arch, format) = host_selector();
        Self {
            base_url: base_url(),
            product: PRODUCT.to_string(),
            channel: DEFAULT_CHANNEL.to_string(),
            platform: platform.to_string(),
            arch: arch.to_string(),
            format: format.to_string(),
            trusted_keys: trusted_keys(),
            timeout: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Deserialize)]
struct LatestResponse {
    version: String,
    release_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    pub platform: String,
    pub arch: String,
    pub format: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseManifest {
    schema: u32,
    product: String,
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    min_supported: Option<String>,
    artifacts: Vec<Artifact>,
}

/// A verified newer release, ready to be downloaded.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub version: String,
    pub notes: String,
    pub min_supported: Option<String>,
    pub artifact: Artifact,
    pub download_url: String,
    pub base_url: String,
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(format!("rms-memory/{}", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .build()
        .into()
}

fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        bail!("GET {url}: HTTP {status}");
    }
    let mut out = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut out)
        .with_context(|| format!("reading {url}"))?;
    if out.len() as u64 > limit {
        bail!("{url} is larger than {limit} bytes");
    }
    Ok(out)
}

/// Verify a minisign signature over `message` against any of `keys`.
pub fn verify_signature(message: &[u8], signature_text: &str, keys: &[String]) -> Result<()> {
    let signature = minisign_verify::Signature::decode(signature_text)
        .map_err(|e| anyhow!("malformed release signature: {e}"))?;
    for key in keys {
        let parsed = if key.contains('\n') {
            minisign_verify::PublicKey::decode(key)
        } else {
            minisign_verify::PublicKey::from_base64(key.trim())
        };
        let Ok(public_key) = parsed else { continue };
        if public_key.verify(message, &signature, false).is_ok() {
            return Ok(());
        }
    }
    bail!("release signature is not valid for any trusted key")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Pick the artifact for this host from a *verified* manifest.
pub fn select_artifact<'a>(
    manifest_artifacts: &'a [Artifact],
    opts: &UpdateOptions,
) -> Option<&'a Artifact> {
    manifest_artifacts
        .iter()
        .find(|a| a.platform == opts.platform && a.arch == opts.arch && a.format == opts.format)
}

/// Reject file names that could escape the download directory.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// Ask the update center for a newer release and verify its signed manifest.
/// `Ok(None)` means `current` is already the newest (or newer than the published one).
pub fn check(opts: &UpdateOptions, current: &str) -> Result<Option<Candidate>> {
    let agent = agent(opts.timeout);
    let url = format!(
        "{}/v1/{}/latest?channel={}&platform={}&arch={}&format={}&current={}",
        opts.base_url, opts.product, opts.channel, opts.platform, opts.arch, opts.format, current
    );
    let mut response = agent
        .get(&url)
        .call()
        .with_context(|| format!("could not reach the update center ({})", opts.base_url))?;
    match response.status().as_u16() {
        204 | 404 => return Ok(None),
        200 => {}
        other => bail!("update center answered HTTP {other}"),
    }
    let latest: LatestResponse = response
        .body_mut()
        .read_json()
        .context("unexpected update center response")?;
    if !crate::update_check::is_newer(&latest.version, current) {
        return Ok(None);
    }

    // Trust only the signed manifest, never the locator response.
    let manifest_bytes = get_bytes(&agent, &latest.release_url, MAX_MANIFEST_BYTES)?;
    let signature_url = format!("{}.minisig", latest.release_url);
    let signature_text = String::from_utf8(get_bytes(&agent, &signature_url, 4096)?)
        .context("release signature is not text")?;
    verify_signature(&manifest_bytes, &signature_text, &opts.trusted_keys)?;
    let manifest: ReleaseManifest =
        serde_json::from_slice(&manifest_bytes).context("invalid signed release.json")?;
    if manifest.schema != 1 || manifest.product != opts.product {
        bail!(
            "signed manifest is for `{}` (schema {}), expected `{}`",
            manifest.product,
            manifest.schema,
            opts.product
        );
    }
    if manifest.version != latest.version {
        bail!(
            "update center announced {} but the signed manifest says {}",
            latest.version,
            manifest.version
        );
    }
    if !crate::update_check::is_newer(&manifest.version, current) {
        return Ok(None);
    }
    let artifact = select_artifact(&manifest.artifacts, opts)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "release {} has no build for {}/{} ({})",
                manifest.version,
                opts.platform,
                opts.arch,
                opts.format
            )
        })?;
    if !safe_file_name(&artifact.file) {
        bail!("signed manifest contains an unsafe file name");
    }
    // Served by the update center itself unless the manifest names an https source.
    let download_url = match artifact.source.as_deref() {
        Some(src) if src.starts_with("https://") => src.to_string(),
        // `src=app` labels the download as coming from the built-in updater, so the update
        // center can count it apart from README and website links.
        _ => format!(
            "{}/dl/{}/{}/{}?src=app",
            opts.base_url, opts.product, manifest.version, artifact.file
        ),
    };
    Ok(Some(Candidate {
        version: manifest.version,
        notes: manifest.notes,
        min_supported: manifest.min_supported,
        artifact,
        download_url,
        base_url: opts.base_url.clone(),
    }))
}

/// Binary name inside the release archive.
pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "rms-memory.exe"
    } else {
        "rms-memory"
    }
}

/// A verified binary extracted into a private temp directory.
pub struct Staged {
    pub version: String,
    pub binary: PathBuf,
    _dir: tempfile::TempDir,
}

/// Download the artifact, check size and sha256 against the signed manifest, extract the binary.
pub fn download_and_stage(candidate: &Candidate) -> Result<Staged> {
    let agent = agent(Duration::from_secs(300));
    let size = candidate.artifact.size;
    if size == 0 || size > MAX_ARTIFACT_BYTES {
        bail!("implausible artifact size {size}");
    }
    let bytes = get_bytes(&agent, &candidate.download_url, size)?;
    if bytes.len() as u64 != size {
        bail!(
            "downloaded {} bytes, the signed manifest says {size}",
            bytes.len()
        );
    }
    if !sha256_hex(&bytes).eq_ignore_ascii_case(&candidate.artifact.sha256) {
        bail!("checksum mismatch: the download does not match the signed manifest");
    }
    let dir = tempfile::tempdir().context("temp dir")?;
    let binary = dir.path().join(binary_name());
    extract_binary(&bytes, &candidate.artifact.format, &binary)?;
    Ok(Staged {
        version: candidate.version.clone(),
        binary,
        _dir: dir,
    })
}

/// Extract only the `rms-memory` entry (by file name) from an archive.
pub fn extract_binary(archive: &[u8], format: &str, dest: &Path) -> Result<()> {
    let wanted = binary_name();
    match format {
        "tar.gz" => {
            let decoder = flate2::read::GzDecoder::new(archive);
            let mut tar = tar::Archive::new(decoder);
            for entry in tar.entries()? {
                let mut entry = entry?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let path = entry.path()?.into_owned();
                if path.file_name().is_some_and(|n| n == wanted) {
                    let mut out = std::fs::File::create(dest)?;
                    std::io::copy(&mut entry, &mut out)?;
                    return Ok(());
                }
            }
        }
        "zip" => {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))?;
            for i in 0..zip.len() {
                let mut file = zip.by_index(i)?;
                if file.is_file()
                    && file
                        .enclosed_name()
                        .and_then(|p| p.file_name().map(|n| n == wanted))
                        .unwrap_or(false)
                {
                    let mut out = std::fs::File::create(dest)?;
                    std::io::copy(&mut file, &mut out)?;
                    return Ok(());
                }
            }
        }
        other => bail!("unsupported archive format `{other}`"),
    }
    bail!("`{wanted}` not found in the archive")
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

/// Where a fresh install goes (matches `scripts/install.sh` / `install.ps1`).
pub fn default_install_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(windows) {
        home.join(".rms-memory").join("bin").join(binary_name())
    } else {
        home.join(".local").join("bin").join(binary_name())
    }
}

/// The `rms-memory` binary on PATH or in the default install location, if any.
pub fn find_installed() -> Option<PathBuf> {
    let default = default_install_path();
    if default.is_file() {
        return Some(default);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary_name()))
        .find(|candidate| candidate.is_file())
}

/// Version printed by `<binary> --version` (`rms-memory 1.3.2` -> `1.3.2`).
pub fn binary_version(binary: &Path) -> Result<String> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("running {} --version", binary.display()))?;
    if !output.status.success() {
        bail!("{} --version failed", binary.display());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|word| semver::Version::parse(word.trim_start_matches('v')).is_ok())
        .map(|v| v.trim_start_matches('v').to_string())
        .ok_or_else(|| anyhow!("unexpected --version output: {}", text.trim()))
}

#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub target: PathBuf,
    pub version: String,
    /// Previous binary kept for manual rollback (absent on a fresh install).
    pub backup: Option<PathBuf>,
}

/// Install `staged` at `target`: smoke-test it, then swap atomically keeping a backup.
pub fn install_staged(staged: &Staged, target: &Path) -> Result<InstallOutcome> {
    make_executable(&staged.binary)?;
    let reported = binary_version(&staged.binary)
        .context("the downloaded binary does not run on this machine")?;
    if reported != staged.version {
        bail!(
            "downloaded binary reports version {reported}, expected {}",
            staged.version
        );
    }
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("install path has no parent directory"))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let incoming = dir.join(format!(".{}.new", binary_name()));
    std::fs::copy(&staged.binary, &incoming).with_context(|| {
        format!(
            "writing {} (is the directory writable?)",
            incoming.display()
        )
    })?;
    make_executable(&incoming)?;

    let backup = if target.exists() {
        let backup = dir.join(format!("{}.prev", binary_name()));
        let _ = std::fs::remove_file(&backup);
        // Renaming a running executable is allowed on Unix and Windows; copying over it is not.
        std::fs::rename(target, &backup)
            .with_context(|| format!("moving the current binary to {}", backup.display()))?;
        Some(backup)
    } else {
        None
    };
    if let Err(error) = std::fs::rename(&incoming, target) {
        if let Some(backup) = &backup {
            let _ = std::fs::rename(backup, target);
        }
        let _ = std::fs::remove_file(&incoming);
        return Err(anyhow!(error))
            .context("replacing the binary failed; previous version restored");
    }
    Ok(InstallOutcome {
        target: target.to_path_buf(),
        version: staged.version.clone(),
        backup,
    })
}

/// Restore the backup left by the last update.
pub fn rollback(target: &Path) -> Result<PathBuf> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("install path has no parent directory"))?;
    let backup = dir.join(format!("{}.prev", binary_name()));
    if !backup.is_file() {
        bail!("no backup found at {}", backup.display());
    }
    let broken = dir.join(format!(".{}.rolled-back", binary_name()));
    let _ = std::fs::remove_file(&broken);
    if target.exists() {
        std::fs::rename(target, &broken)?;
    }
    std::fs::rename(&backup, target).context("restoring the backup")?;
    let _ = std::fs::remove_file(&broken);
    Ok(target.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write as _;
    use std::net::TcpListener;
    use std::sync::Arc;

    #[test]
    fn contract_test_vector_verifies() {
        let message = b"rms-update-center cross-language test vector\n";
        let sig = "untrusted comment: signature from rms-release\nRURDz+UCG7OtML+F7ZEG4eBOI5mQhJ8/9PfRU5WP3xPCrJUK84lc2X+pzpULPs9lPSFLmMDmhQIhJyd82DW3cESGwtbpui5PRAA=\ntrusted comment: test vector\nszQrF7goihIwya4BCWJgqiqzjpHhzEZhV9j05ZiNZMMsYRCUBBa70WvGhx/xLS9WDey4Gut6rpUp+ojnCXfMBw==\n";
        let key = "untrusted comment: minisign public key: 30ADB31B02E5CF43\nRWRDz+UCG7OtMKIvOfZ8QtrbOn4C//mDHas4LIScouNCyzSVGq2N7Sr3".to_string();
        verify_signature(message, sig, std::slice::from_ref(&key)).unwrap();
        assert!(verify_signature(b"tampered", sig, std::slice::from_ref(&key)).is_err());
        // A different key must not verify it.
        assert!(verify_signature(message, sig, &trusted_keys()).is_err());
    }

    fn artifact(file: &str, platform: &str, arch: &str, format: &str) -> Artifact {
        Artifact {
            platform: platform.into(),
            arch: arch.into(),
            format: format.into(),
            file: file.into(),
            size: 1,
            sha256: "0".repeat(64),
            source: None,
            urls: vec![],
        }
    }

    #[test]
    fn selects_the_host_artifact_only() {
        let opts = UpdateOptions {
            platform: "linux".into(),
            arch: "arm64".into(),
            format: "tar.gz".into(),
            ..UpdateOptions::default()
        };
        let list = vec![
            artifact("a", "linux", "amd64", "tar.gz"),
            artifact("b", "linux", "arm64", "tar.gz"),
            artifact("c", "darwin", "arm64", "tar.gz"),
        ];
        assert_eq!(select_artifact(&list, &opts).unwrap().file, "b");
        let none = vec![artifact("a", "windows", "amd64", "zip")];
        assert!(select_artifact(&none, &opts).is_none());
    }

    #[test]
    fn rejects_path_traversal_names() {
        assert!(safe_file_name("rms_memory_mcp_1.3.3_x.tar.gz"));
        assert!(!safe_file_name("../evil"));
        assert!(!safe_file_name("a/b"));
        assert!(!safe_file_name(".."));
        assert!(!safe_file_name(""));
    }

    fn tar_gz_with(name: &str, content: &[u8]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, name, content).unwrap();
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn extracts_the_binary_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        let archive = tar_gz_with(binary_name(), b"#!/bin/sh\necho hi\n");
        extract_binary(&archive, "tar.gz", &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"#!/bin/sh\necho hi\n");
        let other = tar_gz_with("something-else", b"x");
        assert!(extract_binary(&other, "tar.gz", &dest).is_err());
        assert!(extract_binary(&archive, "rar", &dest).is_err());
    }

    #[cfg(unix)]
    fn script(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho \"rms-memory {version}\"\n").into_bytes()
    }

    #[cfg(unix)]
    #[test]
    fn install_swaps_keeps_backup_and_rolls_back() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(binary_name());
        std::fs::write(&target, script("1.0.0")).unwrap();
        make_executable(&target).unwrap();

        let stage_dir = tempfile::tempdir().unwrap();
        let binary = stage_dir.path().join(binary_name());
        std::fs::write(&binary, script("2.0.0")).unwrap();
        let staged = Staged {
            version: "2.0.0".into(),
            binary,
            _dir: stage_dir,
        };
        let outcome = install_staged(&staged, &target).unwrap();
        assert_eq!(binary_version(&target).unwrap(), "2.0.0");
        assert!(outcome.backup.as_ref().unwrap().is_file());

        rollback(&target).unwrap();
        assert_eq!(binary_version(&target).unwrap(), "1.0.0");
    }

    #[cfg(unix)]
    #[test]
    fn install_refuses_wrong_version_and_leaves_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(binary_name());
        std::fs::write(&target, script("1.0.0")).unwrap();
        make_executable(&target).unwrap();
        let stage_dir = tempfile::tempdir().unwrap();
        let binary = stage_dir.path().join(binary_name());
        std::fs::write(&binary, script("9.9.9")).unwrap();
        let staged = Staged {
            version: "2.0.0".into(),
            binary,
            _dir: stage_dir,
        };
        assert!(install_staged(&staged, &target).is_err());
        assert_eq!(binary_version(&target).unwrap(), "1.0.0");
    }

    /// Minimal HTTP server: path (with query stripped) -> (status, body).
    fn serve(routes: HashMap<String, (u16, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes = Arc::new(routes);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request.split_whitespace().nth(1).unwrap_or("/");
                    let path = target.split('?').next().unwrap_or("/");
                    let (status, body) = routes
                        .get(path)
                        .cloned()
                        .unwrap_or((404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        format!("http://{addr}")
    }

    struct Fixture {
        opts: UpdateOptions,
        archive: Vec<u8>,
    }

    /// Signed release 2.0.0 for the host, served by a local server.
    fn fixture(tamper_archive: bool, sign_with_trusted: bool) -> Fixture {
        let (platform, arch, format) = host_selector();
        let archive = if format == "zip" {
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut zip = zip::ZipWriter::new(&mut cursor);
                zip.start_file::<_, ()>(binary_name(), zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"binary").unwrap();
                zip.finish().unwrap();
            }
            cursor.into_inner()
        } else {
            tar_gz_with(binary_name(), b"binary")
        };
        let file = format!("rms_memory_mcp_2.0.0_{platform}_{arch}.{format}");
        let manifest = serde_json::json!({
            "schema": 1, "product": PRODUCT, "version": "2.0.0",
            "published_at": "2026-11-10T12:00:00Z", "sequence": 20261110120000u64,
            "channels": ["stable"], "notes": "## Notes\n- new",
            "artifacts": [{
                "platform": platform, "arch": arch, "format": format, "file": file,
                "size": archive.len(), "sha256": sha256_hex(&archive), "source": null
            }]
        })
        .to_string()
        .into_bytes();

        let keypair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let other = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let signer = if sign_with_trusted { &keypair } else { &other };
        let signature = minisign::sign(
            Some(&signer.pk),
            &signer.sk,
            std::io::Cursor::new(&manifest),
            None,
            None,
        )
        .unwrap()
        .to_string();

        let served_archive = if tamper_archive {
            let mut bad = archive.clone();
            let last = bad.len() - 1;
            bad[last] ^= 0xff;
            bad
        } else {
            archive.clone()
        };
        let mut routes = HashMap::new();
        // The server must know its own address for release_url, so reserve it first.
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let base = format!("http://{addr}");
        routes.insert(
            format!("/v1/{PRODUCT}/latest"),
            (
                200,
                serde_json::json!({
                    "version": "2.0.0",
                    "release_url": format!("{base}/v1/{PRODUCT}/2.0.0/release.json")
                })
                .to_string()
                .into_bytes(),
            ),
        );
        routes.insert(format!("/v1/{PRODUCT}/2.0.0/release.json"), (200, manifest));
        routes.insert(
            format!("/v1/{PRODUCT}/2.0.0/release.json.minisig"),
            (200, signature.into_bytes()),
        );
        routes.insert(format!("/dl/{PRODUCT}/2.0.0/{file}"), (200, served_archive));

        let listener = TcpListener::bind(addr).unwrap();
        let routes = Arc::new(routes);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).to_string();
                    let target = request.split_whitespace().nth(1).unwrap_or("/");
                    let path = target.split('?').next().unwrap_or("/");
                    let (status, body) = routes
                        .get(path)
                        .cloned()
                        .unwrap_or((404, b"not found".to_vec()));
                    let head = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                });
            }
        });
        let opts = UpdateOptions {
            base_url: base,
            trusted_keys: vec![keypair.pk.to_box().unwrap().to_string()],
            ..UpdateOptions::default()
        };
        Fixture { opts, archive }
    }

    #[test]
    fn check_accepts_a_correctly_signed_newer_release() {
        let f = fixture(false, true);
        let candidate = check(&f.opts, "1.0.0").unwrap().expect("update expected");
        assert_eq!(candidate.version, "2.0.0");
        assert!(candidate.notes.contains("new"));
        assert!(candidate.download_url.contains("/dl/rms-memory-mcp/2.0.0/"));
        let staged = download_and_stage(&candidate).unwrap();
        assert_eq!(std::fs::read(&staged.binary).unwrap(), b"binary");
        assert_eq!(f.archive.len() as u64, candidate.artifact.size);
        // Already current: nothing to do.
        assert!(check(&f.opts, "2.0.0").unwrap().is_none());
    }

    #[test]
    fn check_rejects_a_manifest_signed_by_an_untrusted_key() {
        let f = fixture(false, false);
        let error = check(&f.opts, "1.0.0").unwrap_err().to_string();
        assert!(error.contains("trusted key"), "{error}");
    }

    #[test]
    fn download_rejects_a_tampered_archive() {
        let f = fixture(true, true);
        let candidate = check(&f.opts, "1.0.0").unwrap().unwrap();
        let error = download_and_stage(&candidate).err().unwrap().to_string();
        assert!(error.contains("checksum"), "{error}");
    }

    #[test]
    fn unknown_product_is_not_an_error() {
        let base = serve(HashMap::new());
        let opts = UpdateOptions {
            base_url: base,
            ..UpdateOptions::default()
        };
        assert!(check(&opts, "1.0.0").unwrap().is_none());
    }
}
