//! Full-text index over the user/assistant turns of the archive (SQLite FTS5).
//!
//! Plain archive: `<archive>/.index.db`. Encrypted archive: the whole database is serialized and
//! encrypted to the archive's public key (`.index.db.age`), so no plaintext of your dialogs is
//! ever written to disk. Updating or searching it needs the passphrase (it is loaded into memory).

use crate::collector::{
    Config, Reader, assemble, encrypt_bytes, load_config, load_state, resolve_logical, write_atomic,
};
use crate::turns::{self, Turn};
use rusqlite::{Connection, MAIN_DB, params};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// Turns per session that fit into one rowid block (`session_id * BLOCK + n`).
const BLOCK: i64 = 1_000_000;

#[derive(Debug, Clone)]
pub struct Hit {
    pub rel: String,
    pub tool: String,
    pub project: String,
    pub role: String,
    pub ts: Option<String>,
    pub snippet: String,
}

fn path(c: &Config) -> PathBuf {
    c.out.join(if c.recipient.is_some() {
        ".index.db.age"
    } else {
        ".index.db"
    })
}

fn e<T: ToString>(x: T) -> String {
    x.to_string()
}

const SCHEMA: &str = "
create table if not exists sessions(
  id integer primary key autoincrement, key text unique not null, rel text not null,
  tool text not null, project text not null, name text not null, hash text not null);
create virtual table if not exists turns_fts using fts5(
  text, sid unindexed, role unindexed, ts unindexed, tokenize = 'unicode61 remove_diacritics 2');
";

/// Opens the index. Encrypted archives are decrypted into memory (needs the passphrase).
fn open(c: &Config, rd: &mut Reader) -> Result<Connection, String> {
    let p = path(c);
    let conn = if c.recipient.is_some() {
        let mut conn = Connection::open_in_memory().map_err(e)?;
        if p.exists() {
            let plain = rd.segment(c, &p)?;
            conn.deserialize_read_exact(MAIN_DB, Cursor::new(&plain), plain.len(), false)
                .map_err(e)?;
        }
        conn
    } else {
        Connection::open(&p).map_err(e)?
    };
    conn.execute_batch(SCHEMA).map_err(e)?;
    Ok(conn)
}

fn save(c: &Config, mut conn: Connection) -> Result<(), String> {
    let Some(recipient) = &c.recipient else {
        return Ok(());
    };
    let data = conn.serialize(MAIN_DB).map_err(e)?;
    let enc = encrypt_bytes(recipient, &data)?;
    write_atomic(&path(c), &enc)
}

fn split_rel(rel: &str) -> (String, String, String) {
    let mut it = rel.splitn(3, '/');
    let a = it.next().unwrap_or_default().to_owned();
    let b = it.next().unwrap_or_default().to_owned();
    (a, b, it.next().unwrap_or_default().to_owned())
}

/// Adds new and changed sessions to the index. Returns (sessions indexed, turns added).
pub fn update(cp: &Path) -> Result<(usize, usize), String> {
    let c = load_config(cp)?;
    let st = load_state(&c);
    let mut rd = Reader::new();
    let mut conn = open(&c, &mut rd)?;
    let (mut sessions, mut total) = (0, 0);
    for (key, ent) in &st.seen {
        let known: Option<(i64, String)> = conn
            .query_row("select id, hash from sessions where key = ?1", [key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .ok();
        if known.as_ref().is_some_and(|(_, h)| *h == ent.hash) {
            continue;
        }
        let (tool, project, name) = split_rel(&ent.rel);
        let turns = match resolve_logical(&c.out.join(&ent.rel), Some(ent.generation))
            .and_then(|segs| assemble(&mut rd, &c, &segs))
        {
            Ok(data) => turns::extract(&tool, &name, &data),
            Err(m) if m == "wrong passphrase" || m.starts_with("no key file") => return Err(m),
            Err(m) => {
                if ent.nseg != 0 || ent.len != 0 {
                    eprintln!("index skip {key}: {m}");
                }
                continue;
            }
        };
        let tx = conn.transaction().map_err(e)?;
        let id = match known {
            Some((id, _)) => {
                tx.execute(
                    "delete from turns_fts where rowid >= ?1 and rowid < ?2",
                    params![id * BLOCK, (id + 1) * BLOCK],
                )
                .map_err(e)?;
                tx.execute(
                    "update sessions set hash = ?2, rel = ?3 where id = ?1",
                    params![id, ent.hash, ent.rel],
                )
                .map_err(e)?;
                id
            }
            None => {
                tx.execute(
                    "insert into sessions(key, rel, tool, project, name, hash) values (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![key, ent.rel, tool, project, name, ent.hash],
                )
                .map_err(e)?;
                tx.last_insert_rowid()
            }
        };
        {
            let mut ins = tx
                .prepare(
                    "insert into turns_fts(rowid, text, sid, role, ts) values (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(e)?;
            for (i, t) in turns.iter().take(BLOCK as usize - 1).enumerate() {
                ins.execute(params![id * BLOCK + i as i64, t.text, id, t.role, t.ts])
                    .map_err(e)?;
            }
        }
        tx.commit().map_err(e)?;
        sessions += 1;
        total += turns.len();
    }
    save(&c, conn)?;
    Ok((sessions, total))
}

/// Turns the user's free text into an FTS5 query: every word must occur, the last one as a prefix.
fn fts_query(q: &str) -> String {
    let words: Vec<String> = q
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    let n = words.len();
    words
        .into_iter()
        .enumerate()
        .map(|(i, w)| if i + 1 == n { format!("{w}*") } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn search(
    cp: &Path,
    query: &str,
    tool: Option<&str>,
    project: Option<&str>,
    limit: usize,
) -> Result<Vec<Hit>, String> {
    let c = load_config(cp)?;
    if !path(&c).exists() {
        return Err("no search index yet: run `rms-memory dialogs index`".into());
    }
    let q = fts_query(query);
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let mut rd = Reader::new();
    let conn = open(&c, &mut rd)?;
    let mut st = conn
        .prepare(
            "select s.rel, s.tool, s.project, f.role, f.ts, snippet(turns_fts, 0, '«', '»', '…', 24)
             from turns_fts f join sessions s on s.id = f.sid
             where turns_fts match ?1 and (?2 is null or s.tool = ?2) and (?3 is null or s.project = ?3)
             order by bm25(turns_fts) limit ?4",
        )
        .map_err(e)?;
    let rows = st
        .query_map(params![q, tool, project, limit as i64], |r| {
            Ok(Hit {
                rel: r.get(0)?,
                tool: r.get(1)?,
                project: r.get(2)?,
                role: r.get(3)?,
                ts: r.get(4)?,
                snippet: r.get(5)?,
            })
        })
        .map_err(e)?;
    Ok(rows.flatten().collect())
}

/// One archived session as Markdown (front matter + one heading per turn).
pub fn read_session(cp: &Path, rel: &str) -> Result<(String, Vec<Turn>), String> {
    let c = load_config(cp)?;
    if rel.contains("..") || rel.starts_with('/') {
        return Err("bad session path".into());
    }
    let (tool, project, name) = split_rel(rel);
    let segs = resolve_logical(&c.out.join(rel), None)?;
    let data = assemble(&mut Reader::new(), &c, &segs)?;
    let turns = turns::extract(&tool, &name, &data);
    Ok((turns::to_markdown(&tool, &project, &name, &turns), turns))
}
