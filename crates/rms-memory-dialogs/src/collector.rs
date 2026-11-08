//! Dialogs collector: copies AI-assistant session logs that already sit on the
//! local disk into one archive tree. It only reads files the tools themselves
//! wrote; it never touches network traffic or the tools' processes.
//!
//! Encryption design: sessions are encrypted to an age X25519 *public* key, so
//! the background collector never needs a passphrase. The matching secret key is
//! stored passphrase-protected (age scrypt); only reading needs the passphrase.

use age::secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Off,
    Collect,
    #[value(alias = "collect_and_serve")]
    CollectAndServe,
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub mode: Mode,
    pub out: PathBuf,
    /// age X25519 public key; when set every collected file is encrypted to it
    pub recipient: Option<String>,
}

/// What is archived for one logical file (a source file or a logical export).
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    /// change signature of the source at the last successful run
    pub(crate) sig: (u64, u64),
    /// bytes of the logical file archived so far
    pub(crate) len: u64,
    /// sha256 (hex) of those bytes: the next run appends only if the prefix is unchanged
    pub(crate) hash: String,
    /// current generation and number of segments in it
    pub(crate) generation: u32,
    pub(crate) nseg: u32,
    /// archive location: <tool>/<project>/<name>
    pub(crate) rel: String,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(default)]
    pub(crate) v: u32,
    pub(crate) seen: BTreeMap<String, Entry>,
}

/// Segments per generation before a compacting rewrite; generations kept when content was rewritten.
const MAX_SEGS: u32 = 48;
const KEEP_GENS: u32 = 5;

fn home() -> PathBuf {
    dirs::home_dir().expect("no home directory")
}

/// Default config location, next to the rest of RMS Memory's state.
pub fn default_config_path() -> PathBuf {
    home().join(".rms-memory/dialogs/config.toml")
}

pub fn load_config(p: &Path) -> Result<Config, String> {
    let s = fs::read_to_string(p)
        .map_err(|e| format!("cannot read {}: {e} (run `init`)", p.display()))?;
    toml::from_str(&s).map_err(|e| format!("bad config: {e}"))
}

pub fn save_config(p: &Path, c: &Config) -> Result<(), String> {
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    fs::write(p, toml::to_string_pretty(c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

type Discover = fn(&mut Vec<Found>);

/// One discovered session file and where it goes in the tree.
type Lazy = Box<dyn FnOnce() -> Result<Vec<u8>, String>>;

enum Kind {
    /// plain file copy
    Copy,
    /// live SQLite database: consistent snapshot instead of raw bytes
    Snapshot,
    /// logical export built on demand from selected tables (never the whole database)
    Export(Lazy),
}

struct Found {
    /// state key (a path for files, a pseudo key like `opencode:<id>` for exports)
    key: String,
    src: PathBuf,
    tool: &'static str,
    project: String,
    name: String,
    kind: Kind,
    /// precomputed change signature for exports
    sig: Option<(u64, u64)>,
}

impl Found {
    fn file(src: PathBuf, tool: &'static str, project: String, name: String, kind: Kind) -> Self {
        Found {
            key: src.to_string_lossy().into_owned(),
            src,
            tool,
            project,
            name,
            kind,
            sig: None,
        }
    }
}

fn sanitize(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() { "unknown".into() } else { t }
}

fn files_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files_with_ext(&p, ext, out);
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// Claude Code: ~/.claude/projects/<project-dir>/<session>.jsonl
fn discover_claude(out: &mut Vec<Found>) {
    let root = home().join(".claude/projects");
    let Ok(rd) = fs::read_dir(&root) else { return };
    for d in rd.flatten().filter(|d| d.path().is_dir()) {
        let project = sanitize(&d.file_name().to_string_lossy());
        let mut v = Vec::new();
        files_with_ext(&d.path(), "jsonl", &mut v);
        for src in v {
            let rel = src
                .strip_prefix(d.path())
                .unwrap_or(&src)
                .to_string_lossy()
                .replace('/', "__");
            out.push(Found::file(
                src,
                "claude-code",
                project.clone(),
                sanitize(&rel),
                Kind::Copy,
            ));
        }
    }
}

/// Project name of a Codex rollout = last path component of the cwd in its first line.
fn codex_project(src: &Path) -> String {
    let mut buf = String::new();
    if let Ok(f) = fs::File::open(src) {
        let _ = f.take(64 * 1024).read_to_string(&mut buf);
    }
    let first = buf.lines().next().unwrap_or("");
    serde_json::from_str::<serde_json::Value>(first)
        .ok()
        .and_then(|v| v["payload"]["cwd"].as_str().map(str::to_owned))
        .and_then(|c| {
            Path::new(&c)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .map(|s| sanitize(&s))
        .unwrap_or_else(|| "unknown".into())
}

/// Codex: ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
fn discover_codex(out: &mut Vec<Found>) {
    let mut v = Vec::new();
    files_with_ext(&home().join(".codex/sessions"), "jsonl", &mut v);
    for src in v {
        let name = sanitize(&src.file_name().unwrap_or_default().to_string_lossy());
        let project = codex_project(&src);
        out.push(Found::file(src, "codex", project, name, Kind::Copy));
    }
}

/// Project of an Antigravity conversation: the first path under `<home>/dev/` mentioned in its transcript.
fn antigravity_project(transcript: &Path) -> String {
    let mut buf = String::new();
    if let Ok(f) = fs::File::open(transcript) {
        let _ = f.take(256 * 1024).read_to_string(&mut buf);
    }
    let marker = format!("{}/dev/", home().display());
    buf.find(&marker)
        .map(|i| {
            buf[i + marker.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .collect::<String>()
        })
        .map(|s| s.trim_end_matches('.').to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| sanitize(&s))
        .unwrap_or_else(|| "unknown".into())
}

/// Google Antigravity (only whitelisted paths; ~/.gemini also holds OAuth credentials, never read):
///   brain/<id>/.system_generated/logs/transcript*.jsonl   readable transcripts
///   brain/<id>/*.md                                      plans / walkthroughs written by the agent
///   conversations/<id>.db                                raw trajectory database (SQLite, live WAL)
fn discover_antigravity(out: &mut Vec<Found>) {
    let root = home().join(".gemini/antigravity");
    let mut projects = BTreeMap::new();
    if let Ok(rd) = fs::read_dir(root.join("brain")) {
        for d in rd.flatten().filter(|d| d.path().is_dir()) {
            let id = sanitize(&d.file_name().to_string_lossy());
            let logs = d.path().join(".system_generated/logs");
            let project = ["transcript_full.jsonl", "transcript.jsonl"]
                .iter()
                .map(|n| logs.join(n))
                .find(|p| p.exists())
                .map(|p| antigravity_project(&p))
                .unwrap_or_else(|| "unknown".into());
            for n in ["transcript_full.jsonl", "transcript.jsonl"] {
                let src = logs.join(n);
                if src.is_file() {
                    out.push(Found::file(
                        src,
                        "antigravity",
                        project.clone(),
                        format!("{id}__{n}"),
                        Kind::Copy,
                    ));
                }
            }
            if let Ok(files) = fs::read_dir(d.path()) {
                for f in files
                    .flatten()
                    .filter(|f| f.path().extension().is_some_and(|e| e == "md"))
                {
                    let n = sanitize(&f.file_name().to_string_lossy());
                    out.push(Found::file(
                        f.path(),
                        "antigravity",
                        project.clone(),
                        format!("{id}__{n}"),
                        Kind::Copy,
                    ));
                }
            }
            projects.insert(id, project);
        }
    }
    if let Ok(rd) = fs::read_dir(root.join("conversations")) {
        for f in rd
            .flatten()
            .filter(|f| f.path().extension().is_some_and(|e| e == "db"))
        {
            let id = sanitize(&f.path().file_stem().unwrap_or_default().to_string_lossy());
            let project = projects
                .get(&id)
                .cloned()
                .unwrap_or_else(|| "unknown".into());
            out.push(Found::file(
                f.path(),
                "antigravity",
                project,
                format!("{id}.db"),
                Kind::Snapshot,
            ));
        }
    }
}

/// Consistent copy of a live SQLite database (WAL included) via VACUUM INTO.
fn snapshot_sqlite(src: &Path) -> Result<Vec<u8>, String> {
    let tmp = std::env::temp_dir().join(format!(
        "dc-snap-{}-{}.db",
        std::process::id(),
        UNIX_EPOCH.elapsed().map_or(0, |d| d.as_nanos())
    ));
    let res = (|| -> Result<Vec<u8>, String> {
        let conn =
            rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy().as_ref()])
            .map_err(|e| e.to_string())?;
        fs::read(&tmp).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_file(&tmp);
    res
}

// ---------------------------------------------------------------------------
// Logical exports from SQLite stores. Only conversation tables are read; the
// stores also hold credentials/tokens (OpenCode `credential`, Cursor `ItemTable`).
// ---------------------------------------------------------------------------

fn open_ro(path: &Path) -> Result<rusqlite::Connection, String> {
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    c.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(c)
}

fn val_json(v: rusqlite::types::ValueRef<'_>) -> serde_json::Value {
    use rusqlite::types::ValueRef::*;
    match v {
        Null => serde_json::Value::Null,
        Integer(i) => i.into(),
        Real(f) => serde_json::json!(f),
        Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        Blob(b) => serde_json::json!(format!("blob:{}", b.len())),
    }
}

/// All rows of a query as `{column: value}` JSON objects.
fn rows_json(
    conn: &rusqlite::Connection,
    sql: &str,
    param: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let mut st = conn.prepare(sql).map_err(|e| e.to_string())?;
    let names: Vec<String> = st.column_names().map(|s| s.to_string()).collect();
    let mut rows = st.query([param]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        let mut m = serde_json::Map::new();
        for (i, n) in names.iter().enumerate() {
            m.insert(
                n.clone(),
                val_json(r.get_ref(i).map_err(|e| e.to_string())?),
            );
        }
        out.push(serde_json::Value::Object(m));
    }
    Ok(out)
}

fn jsonl(lines: Vec<serde_json::Value>) -> Vec<u8> {
    let mut out = Vec::new();
    for l in lines {
        out.extend_from_slice(l.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

fn last_component(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|n| sanitize(&n.to_string_lossy()))
        .unwrap_or_else(|| "unknown".into())
}

/// OpenCode: ~/.local/share/opencode/opencode.db -> one JSONL per session (session, message, part rows).
fn discover_opencode(out: &mut Vec<Found>) {
    let db = home().join(".local/share/opencode/opencode.db");
    if !db.exists() {
        return;
    }
    let Ok(conn) = open_ro(&db) else { return };
    let Ok(mut st) = conn.prepare(
        "select s.id, s.directory, s.time_updated, (select count(*) from message m where m.session_id = s.id) from session s",
    ) else {
        return;
    };
    let rows: Vec<(String, String, i64, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    for (id, dir, updated, nmsg) in rows {
        let db = db.clone();
        let sid = id.clone();
        out.push(Found {
            key: format!("opencode:{id}"),
            src: PathBuf::new(),
            tool: "opencode",
            project: last_component(&dir),
            name: format!("{}.jsonl", sanitize(&id)),
            sig: Some((nmsg as u64, updated as u64)),
            kind: Kind::Export(Box::new(move || {
                let conn = open_ro(&db)?;
                let mut lines = Vec::new();
                for (t, sql) in [
                    ("session", "select * from session where id = ?1"),
                    (
                        "message",
                        "select * from message where session_id = ?1 order by time_created, id",
                    ),
                    (
                        "part",
                        "select * from part where session_id = ?1 order by time_created, id",
                    ),
                ] {
                    for row in rows_json(&conn, sql, &sid)? {
                        lines.push(serde_json::json!({ "table": t, "row": row }));
                    }
                }
                Ok(jsonl(lines))
            })),
        });
    }
}

/// Cursor: globalStorage/state.vscdb, only `composerData:*` and `bubbleId:*` keys of cursorDiskKV.
/// `ItemTable` (settings, auth state) is never read.
fn discover_cursor(out: &mut Vec<Found>) {
    let base = home().join("Library/Application Support/Cursor/User");
    let db = base.join("globalStorage/state.vscdb");
    if !db.exists() {
        return;
    }
    let Ok(conn) = open_ro(&db) else { return };
    // workspaceId -> project folder name
    let mut ws_name: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(rd) = fs::read_dir(base.join("workspaceStorage")) {
        for d in rd.flatten() {
            let Ok(t) = fs::read_to_string(d.path().join("workspace.json")) else {
                continue;
            };
            if let Some(f) = serde_json::from_str::<serde_json::Value>(&t)
                .ok()
                .and_then(|v| v["folder"].as_str().map(str::to_owned))
            {
                ws_name.insert(
                    d.file_name().to_string_lossy().into_owned(),
                    last_component(f.trim_start_matches("file://")),
                );
            }
        }
    }
    let mut project_of: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(mut st) = conn.prepare("select composerId, workspaceId from composerHeaders")
        && let Ok(it) = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })
    {
        for (c, w) in it.flatten() {
            if let Some(n) = w.and_then(|w| ws_name.get(&w).cloned()) {
                project_of.insert(c, n);
            }
        }
    }
    let Ok(mut st) = conn.prepare("select substr(key, 14), length(value) from cursorDiskKV where key >= 'composerData:' and key < 'composerData;'") else { return };
    let comps: Vec<(String, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    for (id, len) in comps {
        let lo = format!("bubbleId:{id}:");
        let hi = format!("bubbleId:{id};");
        let n: i64 = conn
            .query_row(
                "select count(*) from cursorDiskKV where key >= ?1 and key < ?2",
                [&lo, &hi],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let db = db.clone();
        let cid = id.clone();
        out.push(Found {
            key: format!("cursor:{id}"),
            src: PathBuf::new(),
            tool: "cursor",
            project: project_of.get(&id).cloned().unwrap_or_else(|| "unknown".into()),
            name: format!("{}.jsonl", sanitize(&id)),
            sig: Some((n as u64, len as u64)),
            kind: Kind::Export(Box::new(move || {
                let conn = open_ro(&db)?;
                let mut lines = Vec::new();
                let head = rows_json(&conn, "select key, value from cursorDiskKV where key = ?1", &format!("composerData:{cid}"))?;
                let mut st = conn.prepare("select key, value from cursorDiskKV where key >= ?1 and key < ?2 order by key").map_err(|e| e.to_string())?;
                let mut rows = st.query([format!("bubbleId:{cid}:"), format!("bubbleId:{cid};")]).map_err(|e| e.to_string())?;
                lines.extend(head);
                while let Some(r) = rows.next().map_err(|e| e.to_string())? {
                    lines.push(serde_json::json!({
                        "key": val_json(r.get_ref(0).map_err(|e| e.to_string())?),
                        "value": val_json(r.get_ref(1).map_err(|e| e.to_string())?),
                    }));
                }
                Ok(jsonl(lines))
            })),
        });
    }
}

/// Zed: threads/threads.db -> one readable JSON per thread (zstd payload decompressed).
fn discover_zed(out: &mut Vec<Found>) {
    let db = home().join("Library/Application Support/Zed/threads/threads.db");
    if !db.exists() {
        return;
    }
    let Ok(conn) = open_ro(&db) else { return };
    let Ok(mut st) = conn.prepare("select id, folder_paths, updated_at, length(data) from threads")
    else {
        return;
    };
    let rows: Vec<(String, Option<String>, String, i64)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map(|it| it.flatten().collect())
        .unwrap_or_default();
    for (id, folders, updated, len) in rows {
        let db = db.clone();
        let tid = id.clone();
        let stamp = updated
            .bytes()
            .fold(0u64, |h, b| h.wrapping_mul(131).wrapping_add(u64::from(b)));
        let project = folders
            .as_deref()
            .and_then(|f| f.lines().next())
            .map(last_component)
            .unwrap_or_else(|| "unknown".into());
        out.push(Found {
            key: format!("zed:{id}"),
            src: PathBuf::new(),
            tool: "zed",
            project,
            name: format!("{}.json", sanitize(&id)),
            sig: Some((len as u64, stamp)),
            kind: Kind::Export(Box::new(move || {
                let conn = open_ro(&db)?;
                conn.query_row(
                    "select summary, created_at, updated_at, folder_paths, data_type, data from threads where id = ?1",
                    [&tid],
                    |r| {
                        let kind: String = r.get(4)?;
                        let raw: Vec<u8> = r.get(5)?;
                        let body = if kind == "zstd" { zstd::decode_all(&raw[..]).unwrap_or(raw) } else { raw };
                        let thread: serde_json::Value = serde_json::from_slice(&body)
                            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&body).into_owned()));
                        Ok(serde_json::json!({
                            "id": tid, "summary": r.get::<_, String>(0)?, "created_at": r.get::<_, Option<String>>(1)?,
                            "updated_at": r.get::<_, String>(2)?, "folder_paths": r.get::<_, Option<String>>(3)?, "thread": thread,
                        }))
                    },
                )
                .map(|v| v.to_string().into_bytes())
                .map_err(|e| e.to_string())
            })),
        });
    }
}

fn state_path(c: &Config) -> PathBuf {
    c.out.join(".state.json")
}

pub(crate) fn load_state(c: &Config) -> State {
    let st: State = fs::read_to_string(state_path(c))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    State { v: 2, ..st }
}

pub(crate) fn encrypt_bytes(recipient: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let r: age::x25519::Recipient = recipient
        .parse()
        .map_err(|e: &str| format!("bad recipient: {e}"))?;
    let enc = age::Encryptor::with_recipients(std::iter::once(&r as &dyn age::Recipient))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut w = enc.wrap_output(&mut out).map_err(|e| e.to_string())?;
    w.write_all(data).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

pub(crate) fn write_atomic(dest: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = dest.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("part");
    fs::write(&tmp, data).map_err(|e| e.to_string())?;
    fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

fn mtime_secs(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

pub(crate) fn sha_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn seg_name(name: &str, generation: u32, seg: u32, suffix: &str) -> String {
    format!("{name}.g{generation:04}.s{seg:04}{suffix}")
}

/// Remove every segment file of one generation.
fn remove_gen(dir: &Path, name: &str, generation: u32) {
    let prefix = format!("{name}.g{generation:04}.s");
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// Where a logical file lives in the archive.
struct Dest {
    tool: &'static str,
    project: String,
    name: String,
    /// whole-file snapshots are replaced, never kept as older generations
    snapshot: bool,
}

/// Write the new bytes of a logical file as append-only segments.
/// Same prefix -> only the tail becomes a new segment; rewritten content -> new generation.
fn store(
    c: &Config,
    f: &Dest,
    prev: Option<&Entry>,
    sig: (u64, u64),
    data: &[u8],
) -> Result<Entry, String> {
    let suffix = if c.recipient.is_some() { ".age" } else { "" };
    let rel = format!("{}/{}/{}", f.tool, f.project, f.name);
    let dir = c.out.join(f.tool).join(&f.project);
    let put = |generation: u32, seg: u32, bytes: &[u8]| -> Result<(), String> {
        let out = match &c.recipient {
            Some(r) => encrypt_bytes(r, bytes)?,
            None => bytes.to_vec(),
        };
        write_atomic(&dir.join(seg_name(&f.name, generation, seg, suffix)), &out)
    };
    let full = |generation: u32| -> Result<Entry, String> {
        if !data.is_empty() {
            put(generation, 1, data)?;
        }
        Ok(Entry {
            sig,
            len: data.len() as u64,
            hash: sha_hex(data),
            generation,
            nseg: u32::from(!data.is_empty()),
            rel: rel.clone(),
        })
    };
    let Some(e) = prev else { return full(1) };
    let l = e.len as usize;
    let prefix_same = data.len() >= l && sha_hex(&data[..l]) == e.hash;
    if prefix_same && data.len() == l {
        return Ok(Entry { sig, ..e.clone() });
    }
    if prefix_same && e.nseg < MAX_SEGS {
        put(e.generation, e.nseg + 1, &data[l..])?;
        return Ok(Entry {
            sig,
            len: data.len() as u64,
            hash: sha_hex(data),
            nseg: e.nseg + 1,
            ..e.clone()
        });
    }
    // compaction (same content, fewer files) or a rewrite (different content): start a new generation
    let ne = full(e.generation + 1)?;
    let drop_old = prefix_same || f.snapshot;
    if drop_old {
        remove_gen(&dir, &f.name, e.generation);
    } else if ne.generation > KEEP_GENS {
        remove_gen(&dir, &f.name, ne.generation - KEEP_GENS);
    }
    Ok(ne)
}

/// Returns (changed, unchanged, failed). Sources that vanish are kept in the archive.
fn collect_once(c: &Config) -> (usize, usize, usize) {
    let mut found = Vec::new();
    let timing = std::env::var_os("RMS_DIALOGS_TIMING").is_some();
    let sources: [(&str, Discover); 6] = [
        ("claude-code", discover_claude),
        ("codex", discover_codex),
        ("antigravity", discover_antigravity),
        ("opencode", discover_opencode),
        ("cursor", discover_cursor),
        ("zed", discover_zed),
    ];
    for (name, discover) in sources {
        let t = std::time::Instant::now();
        discover(&mut found);
        if timing {
            eprintln!(
                "discover {name}: {:?} ({} items so far)",
                t.elapsed(),
                found.len()
            );
        }
    }
    let t_loop = std::time::Instant::now();
    let mut st = load_state(c);
    let (mut copied, mut same, mut failed) = (0, 0, 0);
    for f in found {
        let key = f.key.clone();
        let sig = match (&f.kind, f.sig) {
            (Kind::Export(_), Some(sig)) => sig,
            _ => {
                let Ok(meta) = fs::metadata(&f.src) else {
                    continue;
                };
                let mut sig = (meta.len(), mtime_secs(&meta));
                if matches!(f.kind, Kind::Snapshot) {
                    // changes of a WAL database land in the -wal file first
                    let mut wal = f.src.clone().into_os_string();
                    wal.push("-wal");
                    if let Ok(wm) = fs::metadata(PathBuf::from(wal)) {
                        sig = (sig.0 + wm.len(), sig.1.max(mtime_secs(&wm)));
                    }
                }
                sig
            }
        };
        if st.seen.get(&key).is_some_and(|e| e.sig == sig) {
            same += 1;
            continue;
        }
        let prev = st.seen.get(&key).cloned();
        let Found {
            src,
            tool,
            project,
            name,
            kind,
            ..
        } = f;
        let snapshot = matches!(kind, Kind::Snapshot);
        let data = match kind {
            Kind::Copy => fs::read(&src).map_err(|e| e.to_string()),
            Kind::Snapshot => snapshot_sqlite(&src),
            Kind::Export(run) => run(),
        };
        let dest = Dest {
            tool,
            project,
            name,
            snapshot,
        };
        match data.and_then(|d| store(c, &dest, prev.as_ref(), sig, &d)) {
            Ok(e) => {
                st.seen.insert(key, e);
                copied += 1;
            }
            Err(e) => {
                eprintln!("skip {key}: {e}");
                failed += 1;
            }
        }
    }
    if timing {
        eprintln!(
            "process (signature checks + copies): {:?}",
            t_loop.elapsed()
        );
    }
    if let Ok(s) = serde_json::to_string(&st) {
        let _ = write_atomic(&state_path(c), s.as_bytes());
    }
    (copied, same, failed)
}

fn ask_pass(prompt: &str) -> Result<SecretString, String> {
    if let Ok(p) = std::env::var("RMS_DIALOGS_PASSPHRASE") {
        return Ok(SecretString::from(p));
    }
    rpassword::prompt_password(prompt)
        .map(SecretString::from)
        .map_err(|e| e.to_string())
}

fn key_file(c: &Config) -> PathBuf {
    c.out.join(".keys/identity.age")
}

fn init_encryption(c: &mut Config) -> Result<(), String> {
    let id = age::x25519::Identity::generate();
    let pass = ask_pass("New passphrase: ")?;
    if std::env::var("RMS_DIALOGS_PASSPHRASE").is_err() {
        let again = ask_pass("Repeat passphrase: ")?;
        if again.expose_secret() != pass.expose_secret() {
            return Err("passphrases differ".into());
        }
    }
    let secret = id.to_string();
    let recipient = age::scrypt::Recipient::new(pass);
    let enc = age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut w = enc.wrap_output(&mut out).map_err(|e| e.to_string())?;
    w.write_all(secret.expose_secret().as_bytes())
        .map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())?;
    write_atomic(&key_file(c), &out)?;
    c.recipient = Some(id.to_public().to_string());
    Ok(())
}

/// Unlocks the secret key once (passphrase) and decrypts any number of segments with it.
pub(crate) struct Reader {
    id: Option<age::x25519::Identity>,
}

impl Reader {
    pub(crate) fn new() -> Self {
        Reader { id: None }
    }

    pub(crate) fn identity(&mut self, c: &Config) -> Result<&age::x25519::Identity, String> {
        if self.id.is_none() {
            let kf = fs::read(key_file(c)).map_err(|e| format!("no key file: {e}"))?;
            let pass = ask_pass("Passphrase: ")?;
            let dec = age::Decryptor::new(&kf[..]).map_err(|e| e.to_string())?;
            let sid = age::scrypt::Identity::new(pass);
            let mut r = dec
                .decrypt(std::iter::once(&sid as &dyn age::Identity))
                .map_err(|_| "wrong passphrase".to_string())?;
            let mut secret = String::new();
            r.read_to_string(&mut secret).map_err(|e| e.to_string())?;
            self.id = Some(secret.trim().parse().map_err(|e: &str| e.to_string())?);
        }
        Ok(self.id.as_ref().expect("identity set above"))
    }

    pub(crate) fn segment(&mut self, c: &Config, path: &Path) -> Result<Vec<u8>, String> {
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if path.extension().is_none_or(|e| e != "age") {
            return Ok(data);
        }
        let id = self.identity(c)?;
        let dec = age::Decryptor::new(&data[..]).map_err(|e| e.to_string())?;
        let mut r = dec
            .decrypt(std::iter::once(id as &dyn age::Identity))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        r.read_to_end(&mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }
}

/// `<base>.g0001.s0001[.age]` -> (base, generation, segment)
fn parse_seg(file: &str) -> Option<(String, u32, u32)> {
    let n = file.strip_suffix(".age").unwrap_or(file);
    let cut = n.len().checked_sub(12)?;
    let (base, tail) = (n.get(..cut)?, n.get(cut..)?);
    let b = tail.as_bytes();
    if &tail[0..2] != ".g"
        || &tail[6..8] != ".s"
        || !b[2..6].iter().chain(&b[8..12]).all(u8::is_ascii_digit)
    {
        return None;
    }
    Some((
        base.to_owned(),
        tail[2..6].parse().ok()?,
        tail[8..12].parse().ok()?,
    ))
}

/// base name -> generation -> segments (sorted)
type Groups = BTreeMap<String, BTreeMap<u32, Vec<(u32, PathBuf)>>>;

fn group_dir(dir: &Path) -> Groups {
    let mut g: Groups = BTreeMap::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Some((base, generation, seg)) = parse_seg(&e.file_name().to_string_lossy()) {
                g.entry(base)
                    .or_default()
                    .entry(generation)
                    .or_default()
                    .push((seg, e.path()));
            }
        }
    }
    for gens in g.values_mut() {
        for v in gens.values_mut() {
            v.sort();
        }
    }
    g
}

pub(crate) fn assemble(
    rd: &mut Reader,
    c: &Config,
    segs: &[(u32, PathBuf)],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for (i, (n, p)) in segs.iter().enumerate() {
        if *n as usize != i + 1 {
            return Err(format!("segment {} missing before {}", i + 1, p.display()));
        }
        out.extend(rd.segment(c, p)?);
    }
    Ok(out)
}

/// Resolve a user path (a segment file or the logical name) to its segments.
pub(crate) fn resolve_logical(
    path: &Path,
    generation: Option<u32>,
) -> Result<Vec<(u32, PathBuf)>, String> {
    let file = path
        .file_name()
        .ok_or("no file name")?
        .to_string_lossy()
        .into_owned();
    let base = parse_seg(&file).map_or(file, |(b, _, _)| b);
    let dir = path.parent().unwrap_or(Path::new("."));
    let groups = group_dir(dir);
    let gens = groups
        .get(&base)
        .ok_or_else(|| format!("no archived segments for {base}"))?;
    let g = match generation {
        Some(g) => g,
        None => *gens.keys().next_back().ok_or("empty")?,
    };
    gens.get(&g)
        .cloned()
        .ok_or_else(|| format!("generation {g} not found"))
}

fn restore(c: &Config, to: Option<&Path>, native: bool) -> Result<(), String> {
    let mut rd = Reader::new();
    let (mut n, mut skipped) = (0, 0);
    if let Some(root) = to {
        for tool in fs::read_dir(&c.out)
            .map_err(|e| e.to_string())?
            .flatten()
            .filter(|e| e.path().is_dir())
        {
            let tname = tool.file_name().to_string_lossy().into_owned();
            if tname.starts_with('.') {
                continue;
            }
            for proj in fs::read_dir(tool.path())
                .map_err(|e| e.to_string())?
                .flatten()
                .filter(|e| e.path().is_dir())
            {
                for (base, gens) in group_dir(&proj.path()) {
                    let Some(segs) = gens.values().next_back() else {
                        continue;
                    };
                    let data = assemble(&mut rd, c, segs)?;
                    write_atomic(&root.join(&tname).join(proj.file_name()).join(&base), &data)?;
                    n += 1;
                }
            }
        }
    }
    if native {
        let st = load_state(c);
        for (key, e) in &st.seen {
            let dest = Path::new(key);
            if !key.starts_with('/') || dest.exists() {
                skipped += usize::from(dest.exists());
                continue;
            }
            let segs = resolve_logical(&c.out.join(&e.rel), Some(e.generation))?;
            write_atomic(dest, &assemble(&mut rd, c, &segs)?)?;
            n += 1;
        }
    }
    println!(
        "restored {n} files{}",
        if skipped > 0 {
            format!(" ({skipped} already existed, left untouched)")
        } else {
            String::new()
        }
    );
    Ok(())
}

fn verify(c: &Config, deep: bool) -> Result<(), String> {
    let st = load_state(c);
    let mut rd = Reader::new();
    let (mut ok, mut bad) = (0, 0);
    for (key, e) in &st.seen {
        let check = (|| -> Result<(), String> {
            let segs = resolve_logical(&c.out.join(&e.rel), Some(e.generation))?;
            if segs.len() as u32 != e.nseg {
                return Err(format!(
                    "{} segments on disk, {} expected",
                    segs.len(),
                    e.nseg
                ));
            }
            if deep {
                let data = assemble(&mut rd, c, &segs)?;
                if data.len() as u64 != e.len || sha_hex(&data) != e.hash {
                    return Err("content differs from recorded length/hash".into());
                }
            }
            Ok(())
        })();
        // a logical file with zero segments (empty source) has nothing to resolve
        match check {
            Ok(()) => ok += 1,
            Err(_) if e.nseg == 0 && e.len == 0 => ok += 1,
            Err(m) => {
                eprintln!("BAD {key}: {m}");
                bad += 1;
            }
        }
    }
    println!(
        "verified {ok} ok, {bad} bad ({})",
        if deep {
            "deep: decrypted + sha256"
        } else {
            "structure only"
        }
    );
    if bad > 0 {
        Err("verification failed".into())
    } else {
        Ok(())
    }
}

fn count_files(dir: &Path) -> usize {
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                count_files(&p)
            } else if e.file_name().to_string_lossy().starts_with('.') {
                0
            } else {
                1
            }
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Public API used by the `rms-memory dialogs` command and the MCP tools.
// ---------------------------------------------------------------------------

pub const TOOLS: [&str; 6] = [
    "claude-code",
    "codex",
    "antigravity",
    "opencode",
    "cursor",
    "zed",
];

pub fn init(cp: &Path, out: Option<PathBuf>, encrypt: bool) -> Result<(), String> {
    let mut c = Config {
        mode: Mode::Collect,
        out: out.unwrap_or_else(|| home().join("dialogs-archive")),
        recipient: None,
    };
    fs::create_dir_all(&c.out).map_err(|e| e.to_string())?;
    if encrypt {
        init_encryption(&mut c)?;
    }
    save_config(cp, &c)?;
    println!(
        "initialized: archive={} encryption={}",
        c.out.display(),
        c.recipient.is_some()
    );
    Ok(())
}

pub fn set_mode(cp: &Path, mode: Mode) -> Result<(), String> {
    let mut c = load_config(cp)?;
    c.mode = mode;
    save_config(cp, &c)?;
    println!("mode = {mode:?}");
    Ok(())
}

/// One pass (or repeated passes with `watch`). Returns nothing; prints a summary per pass.
pub fn collect(cp: &Path, watch: bool, interval: u64, index: bool) -> Result<(), String> {
    let c = load_config(cp)?;
    loop {
        let c2 = load_config(cp).unwrap_or_else(|_| Config {
            mode: c.mode,
            out: c.out.clone(),
            recipient: c.recipient.clone(),
        });
        if c2.mode == Mode::Off {
            println!("mode=off, nothing collected");
        } else {
            let (n, s, f) = collect_once(&c2);
            println!("copied {n}, unchanged {s}, failed {f}");
            if index {
                let (sessions, turns) = crate::index::update(cp)?;
                println!("indexed {sessions} sessions, {turns} turns");
            }
        }
        if !watch {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
}

pub fn status(cp: &Path) -> Result<(), String> {
    let c = load_config(cp)?;
    println!("mode:       {:?}", c.mode);
    println!("archive:    {}", c.out.display());
    println!(
        "encryption: {}",
        if c.recipient.is_some() {
            "on (age X25519, key passphrase-protected)"
        } else {
            "off (plain copy)"
        }
    );
    println!("served to LLM: {}", c.mode == Mode::CollectAndServe);
    for t in TOOLS {
        println!("  {t}: {} files", count_files(&c.out.join(t)));
    }
    Ok(())
}

pub fn read(cp: &Path, path: &Path, generation: Option<u32>) -> Result<Vec<u8>, String> {
    let c = load_config(cp)?;
    let segs = resolve_logical(path, generation)?;
    assemble(&mut Reader::new(), &c, &segs)
}

pub fn restore_cmd(cp: &Path, to: Option<&Path>, native: bool) -> Result<(), String> {
    if to.is_none() && !native {
        return Err("give --to <folder> and/or --native".into());
    }
    restore(&load_config(cp)?, to, native)
}

pub fn verify_cmd(cp: &Path, deep: bool) -> Result<(), String> {
    verify(&load_config(cp)?, deep)
}
