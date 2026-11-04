//! Reminders: "remind me to call Viktor Petrovich tomorrow at 12".
//!
//! The agent stores a reminder with `rms_remind`. Every later MCP tool response carries a
//! short notice when a reminder is inside one of its windows (default 30/20/10/5 minutes
//! before, plus once when overdue), and the notice tells the agent to pass it on to the
//! user. Nothing runs in the background: the delivery channel is the agent's own replies.
//!
//! Free core: add / list / done / cancel / snooze and the default windows.
//! The paid GUI edits `reminder-settings.json` (windows, enable switch) and views the list;
//! this module only *reads* those settings, so the core never depends on a license.
//!
//! Files live in `~/.rms-memory/` (global, not per vault): a call to Viktor Petrovich is not
//! tied to a project.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_OFFSETS: [u32; 4] = [30, 20, 10, 5];
const MAX_TEXT: usize = 500;
const MAX_PENDING: usize = 200;
/// Overdue reminders keep appearing in `list`, but stop nagging after this long.
const OVERDUE_GRACE_MINUTES: i64 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pending,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reminder {
    pub id: String,
    pub text: String,
    pub due_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub status: Status,
    /// Windows (minutes before due; `0` = overdue notice) already delivered.
    #[serde(default)]
    pub announced: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_offsets")]
    pub offsets_minutes: Vec<u32>,
}

fn default_true() -> bool {
    true
}
fn default_offsets() -> Vec<u32> {
    DEFAULT_OFFSETS.to_vec()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            offsets_minutes: default_offsets(),
        }
    }
}

impl Settings {
    /// Offsets sorted descending, de-duplicated, zero removed, capped to a sane range.
    fn normalized(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .offsets_minutes
            .iter()
            .copied()
            .filter(|m| (1..=7 * 24 * 60).contains(m))
            .collect();
        v.sort_unstable_by(|a, b| b.cmp(a));
        v.dedup();
        v
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn default_location() -> Self {
        Self {
            dir: crate::workspace::base_dir(),
        }
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn reminders_path(&self) -> PathBuf {
        self.dir.join("reminders.json")
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("reminder-settings.json")
    }

    pub fn settings(&self) -> Settings {
        std::fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        write_atomic(&self.settings_path(), &serde_json::to_vec_pretty(settings)?)
    }

    pub fn load(&self) -> Vec<Reminder> {
        std::fs::read_to_string(self.reminders_path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self, all: &[Reminder]) -> Result<()> {
        write_atomic(&self.reminders_path(), &serde_json::to_vec_pretty(all)?)
    }

    pub fn add(&self, text: &str, due_at: DateTime<Utc>, now: DateTime<Utc>) -> Result<Reminder> {
        let text = text.trim();
        if text.is_empty() {
            bail!("reminder text is empty");
        }
        if text.chars().count() > MAX_TEXT {
            bail!("reminder text is longer than {MAX_TEXT} characters");
        }
        if due_at <= now {
            bail!("the reminder time {due_at} is not in the future (now: {now})");
        }
        let mut all = self.load();
        if all.iter().filter(|r| r.status == Status::Pending).count() >= MAX_PENDING {
            bail!("too many pending reminders (limit {MAX_PENDING})");
        }
        let reminder = Reminder {
            id: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            text: text.to_string(),
            due_at,
            created_at: now,
            status: Status::Pending,
            announced: Vec::new(),
        };
        all.push(reminder.clone());
        self.save(&all)?;
        Ok(reminder)
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Reminder) -> Result<()>) -> Result<Reminder> {
        let mut all = self.load();
        let item = all
            .iter_mut()
            .find(|r| r.id == id)
            .with_context(|| format!("no reminder with id `{id}`"))?;
        f(item)?;
        let out = item.clone();
        self.save(&all)?;
        Ok(out)
    }

    pub fn done(&self, id: &str) -> Result<Reminder> {
        self.update(id, |r| {
            r.status = Status::Done;
            Ok(())
        })
    }

    pub fn cancel(&self, id: &str) -> Result<Reminder> {
        self.update(id, |r| {
            r.status = Status::Cancelled;
            Ok(())
        })
    }

    /// Move the reminder to a new time and re-arm all windows.
    pub fn reschedule(
        &self,
        id: &str,
        due_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Reminder> {
        if due_at <= now {
            bail!("the new time {due_at} is not in the future (now: {now})");
        }
        self.update(id, |r| {
            r.due_at = due_at;
            r.status = Status::Pending;
            r.announced.clear();
            Ok(())
        })
    }

    pub fn pending(&self) -> Vec<Reminder> {
        let mut v: Vec<Reminder> = self
            .load()
            .into_iter()
            .filter(|r| r.status == Status::Pending)
            .collect();
        v.sort_by_key(|r| r.due_at);
        v
    }

    /// Notices due at `now`; marks the delivered windows so each fires once.
    ///
    /// If several windows were skipped (the agent was idle), only the tightest crossed
    /// window is announced and the looser ones are marked as delivered too.
    pub fn take_notices(&self, now: DateTime<Utc>) -> Vec<String> {
        let settings = self.settings();
        if !settings.enabled {
            return Vec::new();
        }
        let offsets = settings.normalized();
        let mut all = self.load();
        let mut notices = Vec::new();
        let mut changed = false;
        for r in all.iter_mut().filter(|r| r.status == Status::Pending) {
            let left = r.due_at - now;
            let minutes_left = left.num_seconds().div_euclid(60); // floor; negative = overdue
            if minutes_left > i64::from(offsets.first().copied().unwrap_or(0)) {
                continue;
            }
            if minutes_left <= 0 {
                if r.announced.contains(&0) || -minutes_left > OVERDUE_GRACE_MINUTES {
                    continue;
                }
                r.announced.push(0);
                notices.push(format!(
                    "Reminder is DUE NOW (id {}): \"{}\" (was set for {} UTC).",
                    r.id,
                    r.text,
                    r.due_at.format("%Y-%m-%d %H:%M")
                ));
                changed = true;
                continue;
            }
            // Windows already crossed: offsets >= minutes_left. The tightest is the smallest.
            let crossed: Vec<u32> = offsets
                .iter()
                .copied()
                .filter(|o| i64::from(*o) >= minutes_left)
                .collect();
            let Some(&tightest) = crossed.iter().min() else {
                continue;
            };
            if r.announced.contains(&tightest) {
                continue;
            }
            for o in crossed {
                if !r.announced.contains(&o) {
                    r.announced.push(o);
                }
            }
            notices.push(format!(
                "Reminder in about {} min (id {}): \"{}\" at {} UTC.",
                minutes_left.max(1),
                r.id,
                r.text,
                r.due_at.format("%H:%M")
            ));
            changed = true;
        }
        if changed {
            let _ = self.save(&all);
        }
        notices
    }
}

/// The text appended to a tool response. `None` when there is nothing to announce.
pub fn notice_block(store: &Store, now: DateTime<Utc>) -> Option<String> {
    let notices = store.take_notices(now);
    if notices.is_empty() {
        return None;
    }
    Some(format!(
        "[RMS reminder] Tell the user about this in your next reply, in their language, then continue:\n{}",
        notices
            .iter()
            .map(|n| format!("- {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

/// Parse `due_at` (RFC3339 with an offset) or `in_minutes`.
pub fn resolve_due(
    due_at: Option<&str>,
    in_minutes: Option<i64>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    match (due_at, in_minutes) {
        (Some(text), _) => Ok(DateTime::parse_from_rfc3339(text.trim())
            .with_context(|| {
                format!("`due_at` must be RFC3339 with a UTC offset, e.g. 2026-11-05T12:00:00+03:00 (got `{text}`)")
            })?
            .with_timezone(&Utc)),
        (None, Some(m)) if m > 0 && m <= 366 * 24 * 60 => Ok(now + Duration::minutes(m)),
        (None, Some(_)) => bail!("`in_minutes` must be between 1 and 527040"),
        (None, None) => bail!("give `due_at` (RFC3339 with offset) or `in_minutes`"),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::at(dir.path());
        (dir, s)
    }

    #[test]
    fn windows_fire_once_each_in_order() {
        let (_d, s) = store();
        let now = t("2026-11-05T08:00:00Z");
        s.add("call Viktor Petrovich", t("2026-11-05T12:00:00+03:00"), now)
            .unwrap(); // 09:00Z
        assert!(s.take_notices(t("2026-11-05T08:00:00Z")).is_empty()); // 60 min
        assert_eq!(s.take_notices(t("2026-11-05T08:30:00Z")).len(), 1); // 30
        assert!(s.take_notices(t("2026-11-05T08:31:00Z")).is_empty()); // same window
        assert_eq!(s.take_notices(t("2026-11-05T08:40:30Z")).len(), 1); // 19.5 -> 20 window
        assert_eq!(s.take_notices(t("2026-11-05T08:50:00Z")).len(), 1); // 10
        assert_eq!(s.take_notices(t("2026-11-05T08:55:00Z")).len(), 1); // 5
        assert!(s.take_notices(t("2026-11-05T08:57:00Z")).is_empty());
        let due = s.take_notices(t("2026-11-05T09:00:00Z"));
        assert_eq!(due.len(), 1);
        assert!(due[0].contains("DUE NOW"));
        assert!(s.take_notices(t("2026-11-05T09:10:00Z")).is_empty());
    }

    #[test]
    fn idle_agent_gets_one_tightest_notice_not_four() {
        let (_d, s) = store();
        let now = t("2026-11-05T08:00:00Z");
        s.add("x", t("2026-11-05T09:00:00Z"), now).unwrap();
        // First contact only at 8:52 (8 min left): window 10 is the tightest crossed.
        let n = s.take_notices(t("2026-11-05T08:52:00Z"));
        assert_eq!(n.len(), 1);
        assert!(n[0].contains("about 8 min"), "{}", n[0]);
        // 30/20/10 are consumed; 5 still fires later.
        assert!(s.take_notices(t("2026-11-05T08:53:00Z")).is_empty());
        assert_eq!(s.take_notices(t("2026-11-05T08:56:00Z")).len(), 1);
    }

    #[test]
    fn done_cancelled_and_disabled_are_silent() {
        let (_d, s) = store();
        let now = t("2026-11-05T08:00:00Z");
        let r = s.add("a", t("2026-11-05T08:20:00Z"), now).unwrap();
        s.done(&r.id).unwrap();
        assert!(s.take_notices(t("2026-11-05T08:15:00Z")).is_empty());
        let r2 = s.add("b", t("2026-11-05T08:20:00Z"), now).unwrap();
        s.save_settings(&Settings {
            enabled: false,
            ..Settings::default()
        })
        .unwrap();
        assert!(s.take_notices(t("2026-11-05T08:15:00Z")).is_empty());
        s.save_settings(&Settings::default()).unwrap();
        s.cancel(&r2.id).unwrap();
        assert!(s.take_notices(t("2026-11-05T08:15:00Z")).is_empty());
        assert!(s.pending().is_empty());
    }

    #[test]
    fn custom_offsets_from_gui_settings() {
        let (_d, s) = store();
        s.save_settings(&Settings {
            enabled: true,
            offsets_minutes: vec![60, 0, 5, 5, 99999999],
        })
        .unwrap();
        let now = t("2026-11-05T08:00:00Z");
        s.add("x", t("2026-11-05T10:00:00Z"), now).unwrap();
        assert!(s.take_notices(t("2026-11-05T08:30:00Z")).is_empty()); // 90 min: no window yet
        assert_eq!(s.take_notices(t("2026-11-05T09:00:00Z")).len(), 1); // 60
        assert_eq!(s.take_notices(t("2026-11-05T09:56:00Z")).len(), 1); // 4 -> 5
    }

    #[test]
    fn snooze_rearms_and_past_times_are_rejected() {
        let (_d, s) = store();
        let now = t("2026-11-05T08:00:00Z");
        assert!(s.add("x", t("2026-11-05T07:59:00Z"), now).is_err());
        assert!(s.add("   ", t("2026-11-05T09:00:00Z"), now).is_err());
        let r = s.add("x", t("2026-11-05T08:20:00Z"), now).unwrap();
        assert_eq!(s.take_notices(t("2026-11-05T08:15:00Z")).len(), 1);
        s.reschedule(&r.id, t("2026-11-05T08:50:00Z"), t("2026-11-05T08:16:00Z"))
            .unwrap();
        assert!(s.load()[0].announced.is_empty());
        assert_eq!(s.take_notices(t("2026-11-05T08:30:00Z")).len(), 1);
        assert!(s.reschedule(&r.id, t("2026-11-05T08:00:00Z"), now).is_err());
    }

    #[test]
    fn overdue_is_announced_once_then_stops_after_grace() {
        let (_d, s) = store();
        let now = t("2026-11-05T08:00:00Z");
        s.add("x", t("2026-11-05T08:10:00Z"), now).unwrap();
        // Agent returns long after: one overdue notice.
        let n = s.take_notices(t("2026-11-05T08:40:00Z"));
        assert_eq!(n.len(), 1);
        assert!(n[0].contains("DUE NOW"));
        assert!(s.take_notices(t("2026-11-05T08:41:00Z")).is_empty());
        let (_d2, s2) = store();
        s2.add("old", t("2026-11-05T08:10:00Z"), now).unwrap();
        assert!(s2.take_notices(t("2026-11-05T20:00:00Z")).is_empty()); // beyond grace
    }

    #[test]
    fn resolve_due_variants() {
        let now = t("2026-11-05T08:00:00Z");
        assert_eq!(
            resolve_due(Some("2026-11-06T12:00:00+03:00"), None, now).unwrap(),
            t("2026-11-06T09:00:00Z")
        );
        assert_eq!(
            resolve_due(None, Some(45), now).unwrap(),
            t("2026-11-05T08:45:00Z")
        );
        assert!(resolve_due(Some("tomorrow at 12"), None, now).is_err());
        assert!(resolve_due(None, None, now).is_err());
        assert!(resolve_due(None, Some(0), now).is_err());
    }

    #[test]
    fn notice_block_tells_the_agent_to_relay() {
        let (_d, s) = store();
        let now = t("2026-11-05T08:00:00Z");
        s.add("call Viktor Petrovich", t("2026-11-05T08:25:00Z"), now)
            .unwrap();
        let block = notice_block(&s, t("2026-11-05T08:00:00Z")).expect("in the 30 window");
        assert!(block.contains("Viktor Petrovich"));
        assert!(block.contains("Tell the user"));
        assert!(notice_block(&s, t("2026-11-05T08:00:30Z")).is_none());
    }
}
