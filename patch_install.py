import re

with open("src/install.rs", "r") as f:
    content = f.read()

# 1. Fix OpenCode key
content = content.replace(
    'IdeConfig::new("OpenCode", vec![\n            "Library/Application Support/opencode/opencode.json",\n            "Library/Application Support/ai.opencode.desktop/settings.json",\n            ".config/opencode/opencode.json",\n        ], "mcpServers"),',
    'IdeConfig::new("OpenCode", vec![\n            "Library/Application Support/opencode/opencode.json",\n            "Library/Application Support/ai.opencode.desktop/settings.json",\n            ".config/opencode/opencode.json",\n        ], "mcp"),'
)

# 2. Add JSONC safe patching function
patch_fn = """
fn strip_json_comments(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut in_string = false;
    let mut in_comment = false;
    let mut in_multiline_comment = false;
    let mut chars = json.chars().peekable();
    
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\\\' {
                if let Some(next_c) = chars.next() {
                    out.push(next_c);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if in_comment {
            if c == '\\n' {
                in_comment = false;
                out.push(c);
            } else {
                out.push(' ');
            }
            continue;
        }
        if in_multiline_comment {
            if c == '*' {
                if let Some(&'/') = chars.peek() {
                    chars.next();
                    in_multiline_comment = false;
                    out.push_str("  ");
                } else {
                    out.push(' ');
                }
            } else if c == '\\n' {
                out.push('\\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if c == '/' {
            if let Some(&'/') = chars.peek() {
                chars.next();
                in_comment = true;
                out.push_str("  ");
                continue;
            } else if let Some(&'*') = chars.peek() {
                chars.next();
                in_multiline_comment = true;
                out.push_str("  ");
                continue;
            }
        }
        if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

fn inject_jsonc(original: &str, key: &str, tool_name: &str, tool_config: &serde_json::Value) -> Option<String> {
    let stripped = strip_json_comments(original);
    let mut json = serde_json::from_str::<serde_json::Value>(&stripped).ok()?;
    
    let obj = json.as_object_mut()?;
    if let Some(mcp) = obj.get(key) {
        if let Some(mcp_obj) = mcp.as_object() {
            if mcp_obj.contains_key(tool_name) {
                // Already configured
                return Some(original.to_string());
            }
        }
    }
    
    let tool_config_str = serde_json::to_string_pretty(tool_config).unwrap();
    // indent it
    let tool_config_str = tool_config_str.replace("\\n", "\\n      ");
    let injection = format!("\\"{}\\": {}", tool_name, tool_config_str);
    
    if obj.contains_key(key) {
        // Simple regex to find "key": {
        let pattern = format!(r#"("{}"\\s*:\\s*\\{{)"#, key);
        let re = regex::Regex::new(&pattern).unwrap();
        if let Some(mat) = re.find(original) {
            let mut patched = original.to_string();
            // check if the dictionary is empty
            let after_brace = &original[mat.end()..];
            let just_whitespace_then_close = after_brace.trim_start().starts_with("}");
            if just_whitespace_then_close {
                patched.insert_str(mat.end(), &format!("\\n      {}\\n    ", injection));
            } else {
                patched.insert_str(mat.end(), &format!("\\n      {},", injection));
            }
            return Some(patched);
        }
    } else {
        // Insert right before the last closing brace
        if let Some(last_brace) = original.rfind('}') {
            let mut patched = original[..last_brace].to_string();
            let trimmed = patched.trim_end();
            patched.truncate(trimmed.len());
            let needs_comma = !patched.ends_with(',') && !patched.ends_with('{');
            if needs_comma {
                patched.push(',');
            }
            patched.push_str(&format!("\\n  \\"{}\\": {{\\n    {}\\n  }}\\n}}", key, injection.replace("      ", "    ")));
            return Some(patched);
        }
    }
    
    None
}
"""

if "fn strip_json_comments" not in content:
    content += "\n" + patch_fn

with open("src/install.rs", "w") as f:
    f.write(content)
