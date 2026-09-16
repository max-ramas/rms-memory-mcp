//! Local heuristics for companion GUI / AI presence (no network, no secrets).

use serde::Serialize;
use std::path::PathBuf;

const ENV_GUI: &str = "RMS_MEMORY_GUI_INSTALLED";
const ENV_AI: &str = "RMS_MEMORY_AI_CONFIGURED";
const RELEASES_URL: &str = "https://github.com/max-ramas/rms-memory-mcp/releases";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpsellMode {
    SoftYellow,
    InfoGray,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanionStatus {
    pub gui_installed: bool,
    pub ai_configured: bool,
}

impl CompanionStatus {
    pub fn upsell_mode(&self) -> UpsellMode {
        if self.gui_installed {
            UpsellMode::InfoGray
        } else {
            UpsellMode::SoftYellow
        }
    }

    pub fn gui_label(&self) -> &'static str {
        if self.gui_installed {
            "installed"
        } else {
            "not installed"
        }
    }

    pub fn ai_label(&self) -> &'static str {
        if self.ai_configured {
            "configured"
        } else {
            "not configured"
        }
    }
}

/// Detect companion state. Env overrides win (for tests / CI):
/// `RMS_MEMORY_GUI_INSTALLED=0|1` and `RMS_MEMORY_AI_CONFIGURED=0|1`.
pub fn detect() -> CompanionStatus {
    CompanionStatus {
        gui_installed: env_bool(ENV_GUI).unwrap_or_else(detect_gui_installed),
        ai_configured: env_bool(ENV_AI).unwrap_or_else(detect_ai_configured),
    }
}

fn env_bool(name: &str) -> Option<bool> {
    match std::env::var(name).ok()?.trim() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn detect_gui_installed() -> bool {
    gui_candidate_paths().into_iter().any(|path| path.exists())
}

fn gui_candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(custom) = std::env::var("RMS_MEMORY_GUI_PATH") {
        let trimmed = custom.trim();
        if !trimmed.is_empty() {
            paths.push(PathBuf::from(trimmed));
        }
    }
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Applications/RMS Memory.app"));
        paths.push(PathBuf::from("/Applications/rms_memory_gui.app"));
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join("Applications/RMS Memory.app"));
            paths.push(home.join("Applications/rms_memory_gui.app"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        paths.push(PathBuf::from("/usr/bin/rms-memory-gui"));
        paths.push(PathBuf::from("/usr/local/bin/rms-memory-gui"));
        paths.push(PathBuf::from("/opt/RMS Memory/rms-memory-gui"));
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".local/bin/rms-memory-gui"));
            paths.push(home.join(".local/share/applications/rms-memory-gui.desktop"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            paths.push(
                PathBuf::from(pf)
                    .join("RMS Memory")
                    .join("rms-memory-gui.exe"),
            );
        }
        if let Some(local) = dirs::data_local_dir() {
            paths.push(
                local
                    .join("Programs")
                    .join("RMS Memory")
                    .join("rms-memory-gui.exe"),
            );
        }
    }
    paths
}

/// AI is "configured" only when we have a non-secret local signal:
/// GUI `ai-settings.json` with a non-empty `lastTestedAt`/`last_tested_at`,
/// or a local provider (`ollama` / `lmstudio`) with settings present. Never
/// reads keyring secrets. GUI serializes camelCase (`rename_all = "camelCase"`).
fn detect_ai_configured() -> bool {
    let path = ai_settings_path();
    let Some(path) = path else {
        return false;
    };
    if !path.exists() {
        return false;
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    if settings_string(&value, &["lastTestedAt", "last_tested_at"])
        .is_some_and(|s| !s.trim().is_empty())
    {
        return true;
    }
    let provider = settings_string(&value, &["providerId", "provider_id"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(provider.as_str(), "ollama" | "lmstudio" | "lm_studio")
}

fn settings_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = value.get(*key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

fn ai_settings_path() -> Option<PathBuf> {
    Some(
        dirs::data_local_dir()?
            .join("rms-memory-gui")
            .join("ai-settings.json"),
    )
}

pub fn format_status_banner(status: &CompanionStatus) -> String {
    let mut out = format!(
        "Status  GUI: {} · AI: {}\n",
        status.gui_label(),
        status.ai_label()
    );
    if !status.gui_installed {
        out.push_str(
            "        Companion GUI unlocks visual graph, editor, BYOK Organizer — optional.\n",
        );
        out.push_str(&format!("        {RELEASES_URL}\n"));
    } else if !status.ai_configured {
        out.push_str(
            "        AI features (Organizer, AI Wiki) need a provider key in the GUI — optional.\n",
        );
    } else {
        out.push_str("        Companion GUI + AI available locally.\n");
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureLabel {
    None,
    Gui,
    Ai,
    GuiAi,
}

impl FeatureLabel {
    pub fn as_tag(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Gui => Some("GUI"),
            Self::Ai => Some("AI"),
            Self::GuiAi => Some("GUI+AI"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeatureRow {
    pub name: &'static str,
    pub kind: &'static str,
    pub label: FeatureLabel,
    pub summary: &'static str,
    pub cli_behavior: &'static str,
}

pub fn feature_catalog() -> Vec<FeatureRow> {
    vec![
        FeatureRow {
            name: "search / write / read",
            kind: "mcp_core",
            label: FeatureLabel::None,
            summary: "Core vault memory loop",
            cli_behavior: "command",
        },
        FeatureRow {
            name: "durable graph (rms_graph / rms-memory graph)",
            kind: "mcp_core",
            label: FeatureLabel::None,
            summary: "Neighbors, path, snapshot, mutations via MCP and CLI",
            cli_behavior: "command",
        },
        FeatureRow {
            name: "doctor / reindex / sync",
            kind: "mcp_core",
            label: FeatureLabel::None,
            summary: "Health checks and index rebuild via MCP + CLI",
            cli_behavior: "command",
        },
        FeatureRow {
            name: "checkpoints / prune / file-history",
            kind: "mcp_core",
            label: FeatureLabel::None,
            summary: "Session continuity and lifecycle",
            cli_behavior: "command",
        },
        FeatureRow {
            name: "config / projects / gc",
            kind: "cli_config",
            label: FeatureLabel::None,
            summary: "Local configuration and maintenance",
            cli_behavior: "command",
        },
        FeatureRow {
            name: "Visual GraphView",
            kind: "gui_visual",
            label: FeatureLabel::Gui,
            summary: "WebGL graph explorer (not in CLI)",
            cli_behavior: "catalog_only",
        },
        FeatureRow {
            name: "Visual Markdown editor",
            kind: "gui_visual",
            label: FeatureLabel::Gui,
            summary: "Desktop editor with link resolution",
            cli_behavior: "catalog_only",
        },
        FeatureRow {
            name: "BYOK Organizer",
            kind: "gui_ai",
            label: FeatureLabel::GuiAi,
            summary: "Bring-your-own-key vault organization",
            cli_behavior: "catalog_only",
        },
        FeatureRow {
            name: "AI Wiki proposals",
            kind: "gui_ai",
            label: FeatureLabel::GuiAi,
            summary: "Guided wiki generation with human apply",
            cli_behavior: "catalog_only",
        },
        FeatureRow {
            name: "Spend UI",
            kind: "gui_visual",
            label: FeatureLabel::Gui,
            summary: "Local spend visibility in the desktop app",
            cli_behavior: "catalog_only",
        },
    ]
}

const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_GRAY: &str = "\x1b[90m";
const ANSI_RESET: &str = "\x1b[0m";

pub fn format_features_catalog(status: &CompanionStatus, color: bool) -> String {
    let mut out = format_status_banner(status);
    out.push('\n');
    out.push_str("Features\n");
    let mode = status.upsell_mode();
    for row in feature_catalog() {
        let tag = match row.label.as_tag() {
            Some(tag) => {
                let styled = if color {
                    match mode {
                        UpsellMode::SoftYellow => format!("{ANSI_YELLOW}[{tag}]{ANSI_RESET}"),
                        UpsellMode::InfoGray => format!("{ANSI_GRAY}[{tag}]{ANSI_RESET}"),
                    }
                } else {
                    format!("[{tag}]")
                };
                format!(" {styled}")
            }
            None => String::new(),
        };
        out.push_str(&format!(
            "  · {}{} — {} ({})\n",
            row.name, tag, row.summary, row.kind
        ));
    }
    out.push_str("\nMCP core is never paywalled. GUI/AI tags are informational only.\n");
    out
}

pub fn color_enabled() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TERM").is_none_or(|t| t != "dumb")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsell_mode_follows_gui_install() {
        assert_eq!(
            CompanionStatus {
                gui_installed: false,
                ai_configured: false,
            }
            .upsell_mode(),
            UpsellMode::SoftYellow
        );
        assert_eq!(
            CompanionStatus {
                gui_installed: true,
                ai_configured: false,
            }
            .upsell_mode(),
            UpsellMode::InfoGray
        );
    }

    #[test]
    fn banner_includes_cta_only_when_gui_absent() {
        let missing = format_status_banner(&CompanionStatus {
            gui_installed: false,
            ai_configured: false,
        });
        assert!(missing.contains("not installed"));
        assert!(missing.contains(RELEASES_URL));
        let installed = format_status_banner(&CompanionStatus {
            gui_installed: true,
            ai_configured: true,
        });
        assert!(installed.contains("installed"));
        assert!(installed.contains("configured"));
        assert!(!installed.contains(RELEASES_URL));
    }

    #[test]
    fn env_overrides_drive_detect() {
        // SAFETY: test-only env mutation; serial within this test.
        unsafe {
            std::env::set_var(ENV_GUI, "0");
            std::env::set_var(ENV_AI, "1");
        }
        let status = detect();
        assert!(!status.gui_installed);
        assert!(status.ai_configured);
        unsafe {
            std::env::remove_var(ENV_GUI);
            std::env::remove_var(ENV_AI);
        }
    }

    #[test]
    fn catalog_marks_visual_graph_as_gui_only() {
        let row = feature_catalog()
            .into_iter()
            .find(|r| r.name.contains("GraphView"))
            .expect("GraphView row");
        assert_eq!(row.label, FeatureLabel::Gui);
        assert_eq!(row.cli_behavior, "catalog_only");
        assert_eq!(row.kind, "gui_visual");
    }

    #[test]
    fn detects_gui_camel_case_ai_settings() {
        let value: serde_json::Value = serde_json::from_str(
            r#"{"providerId":"openai","lastTestedAt":"2026-09-15T12:00:00Z"}"#,
        )
        .unwrap();
        assert!(
            settings_string(&value, &["lastTestedAt", "last_tested_at"])
                .is_some_and(|s| !s.is_empty())
        );
        assert_eq!(
            settings_string(&value, &["providerId", "provider_id"]).as_deref(),
            Some("openai")
        );
    }

    #[test]
    fn snake_and_camel_provider_keys_both_read() {
        let snake: serde_json::Value = serde_json::from_str(r#"{"provider_id":"ollama"}"#).unwrap();
        let camel: serde_json::Value =
            serde_json::from_str(r#"{"providerId":"lmstudio"}"#).unwrap();
        assert_eq!(
            settings_string(&snake, &["providerId", "provider_id"]).as_deref(),
            Some("ollama")
        );
        assert_eq!(
            settings_string(&camel, &["providerId", "provider_id"]).as_deref(),
            Some("lmstudio")
        );
    }
}
