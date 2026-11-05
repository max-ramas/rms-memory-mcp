//! Rare, local hints about the optional GUI, shown only in an interactive terminal.
//!
//! The rules keep it polite: at most one hint per 14 days, never in `--json` output, never when
//! stdout is not a terminal (scripts, CI, agents), never once the GUI is installed, and
//! `RMS_MEMORY_NO_TIPS=1` turns it off for good. State is one timestamp in `~/.rms-memory/tips.json`;
//! nothing is sent anywhere.

use chrono::{DateTime, Duration, Utc};
use std::io::IsTerminal;
use std::path::PathBuf;

const MIN_GAP_DAYS: i64 = 14;
pub const OFF_ENV: &str = "RMS_MEMORY_NO_TIPS";

pub const GUI_TIP: &str = "Tip: the optional RMS Memory GUI adds a visual graph, an editor, reminders and an AI organizer (14-day trial).\n     Install and open it with: rms-memory gui    (hide tips: RMS_MEMORY_NO_TIPS=1)";

fn state_path() -> PathBuf {
    rms_memory_core::workspace::base_dir().join("tips.json")
}

fn disabled_by_env() -> bool {
    std::env::var(OFF_ENV)
        .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// Whether a hint is due, given the time of the last one. Pure, for tests.
pub fn is_due(last_shown: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_shown.is_none_or(|last| now - last >= Duration::days(MIN_GAP_DAYS))
}

fn read_last_shown(path: &std::path::Path) -> Option<DateTime<Utc>> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let text = value.get("gui_last_shown")?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn write_last_shown(path: &std::path::Path, now: DateTime<Utc>) {
    let value = serde_json::json!({ "gui_last_shown": now.to_rfc3339() });
    let _ = std::fs::write(path, value.to_string());
}

/// Print the GUI hint if it is due and appropriate. `json_output` suppresses it.
pub fn maybe_show_gui_tip(json_output: bool) {
    if json_output
        || disabled_by_env()
        || !std::io::stdout().is_terminal()
        || crate::companion_status::detect().gui_installed
    {
        return;
    }
    let path = state_path();
    let now = Utc::now();
    if !is_due(read_last_shown(&path), now) {
        return;
    }
    println!("\n{GUI_TIP}");
    write_last_shown(&path, now);
}

pub const GUI_OFFER: &str = "For your convenience: the optional RMS Memory GUI gives you a visual graph, an editor, reminders and an AI organizer.\nIt starts with a 14-day free trial, and the MCP works fully without it.";

/// After `rms-memory init`: offer to install the GUI right now. Asked at most once per 14 days
/// (shared with the other hints, so setting up several projects does not repeat it), only in an
/// interactive terminal, never when the GUI is installed or `RMS_MEMORY_NO_TIPS=1`. A failed
/// install is reported but never fails `init`.
pub async fn offer_gui_after_init() {
    if disabled_by_env()
        || !std::io::stdout().is_terminal()
        || !std::io::stdin().is_terminal()
        || crate::companion_status::detect().gui_installed
    {
        return;
    }
    let path = state_path();
    let now = Utc::now();
    if !is_due(read_last_shown(&path), now) {
        return;
    }
    // Count the question as shown before asking, so a decline is remembered too.
    write_last_shown(&path, now);
    println!("\n{GUI_OFFER}");
    let accept = dialoguer::Confirm::new()
        .with_prompt("Install the GUI now?")
        .default(false)
        .interact()
        .unwrap_or(false);
    if !accept {
        println!("No problem. Whenever you want it: rms-memory gui");
        return;
    }
    let args = crate::commands::gui::GuiArgs {
        download_only: false,
        no_open: false,
        yes: true,
        json: false,
    };
    if let Err(error) = args.run(None).await {
        eprintln!(
            "Could not install the GUI now: {error:#}\nYou can try later with: rms-memory gui"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tip_is_due_only_after_two_weeks() {
        let now = Utc::now();
        assert!(is_due(None, now));
        assert!(!is_due(Some(now - Duration::days(1)), now));
        assert!(!is_due(Some(now - Duration::days(13)), now));
        assert!(is_due(Some(now - Duration::days(14)), now));
        assert!(is_due(Some(now - Duration::days(90)), now));
    }

    #[test]
    fn state_round_trips_and_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tips.json");
        assert_eq!(read_last_shown(&path), None);
        let now = Utc::now();
        write_last_shown(&path, now);
        let read = read_last_shown(&path).unwrap();
        assert!((read - now).num_seconds().abs() <= 1);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read_last_shown(&path), None);
    }

    #[test]
    fn the_init_offer_mentions_the_trial_and_that_the_mcp_needs_no_gui() {
        assert!(GUI_OFFER.contains("14-day"));
        assert!(GUI_OFFER.contains("without it"));
    }

    #[test]
    fn the_tip_names_the_command_and_the_opt_out() {
        assert!(GUI_TIP.contains("rms-memory gui"));
        assert!(GUI_TIP.contains(OFF_ENV));
    }
}
