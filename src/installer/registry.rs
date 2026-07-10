pub type PayloadBuilder = fn(exe: &str) -> serde_json::Value;

pub fn standard_payload(exe: &str) -> serde_json::Value {
    serde_json::json!({
        "command": exe,
        "args": ["serve"],
        "enabled": true
    })
}

pub fn opencode_payload(exe: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "local",
        "command": [exe, "serve"],
        "enabled": true
    })
}

#[derive(Debug, Clone)]
pub struct IdeConfig {
    pub name: &'static str,
    pub paths: Vec<&'static str>, // relative to home dir
    pub key: &'static str,
    pub build_payload: PayloadBuilder,
}

impl IdeConfig {
    pub fn new(
        name: &'static str,
        paths: Vec<&'static str>,
        key: &'static str,
        build_payload: PayloadBuilder,
    ) -> Self {
        Self {
            name,
            paths,
            key,
            build_payload,
        }
    }
}

pub fn get_ide_registry() -> Vec<IdeConfig> {
    vec![
        IdeConfig::new(
            "Claude Desktop",
            vec![
                "Library/Application Support/Claude/claude_desktop_config.json",
                ".config/Claude/claude_desktop_config.json", // linux
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new("Cursor", vec![".cursor/mcp.json"], "mcpServers", standard_payload),
        IdeConfig::new("Zed", vec![".config/zed/settings.json"], "context_servers", standard_payload),
        IdeConfig::new(
            "VSCode (Roo Cline)",
            vec![
                "Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
                ".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "VSCode (Cline)",
            vec![
                "Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
                ".config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "Windsurf",
            vec![".codeium/windsurf/mcp_config.json"],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "Antigravity IDE (Roo Cline)",
            vec![
                "Library/Application Support/Antigravity IDE/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
                ".config/Antigravity IDE/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "Gemini CLI",
            vec![
                ".gemini/config/mcp_config.json",
                ".gemini/antigravity/mcp_config.json",
                ".gemini/settings.json",
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "QwenCode",
            vec![
                "Library/Application Support/Qwen/settings.json",
                ".config/Qwen/settings.json",
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "OpenCode",
            vec![
                "Library/Application Support/opencode/opencode.json",
                "Library/Application Support/ai.opencode.desktop/settings.json",
                ".config/opencode/opencode.json",
            ],
            "mcp",
            opencode_payload,
        ),
        IdeConfig::new(
            "ZCode",
            vec![
                "Library/Application Support/ZCode/settings.json",
                ".config/ZCode/settings.json",
            ],
            "mcpServers",
            standard_payload,
        ),
        IdeConfig::new(
            "Nova",
            vec![
                "Library/Application Support/Nova/settings.json",
                "Library/Application Support/Nova/Workspaces/Metadata.json",
            ],
            "mcpServers",
            standard_payload,
        ),
    ]
}
