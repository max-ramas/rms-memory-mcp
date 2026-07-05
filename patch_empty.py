with open("src/install.rs", "r") as f:
    content = f.read()

target = "fn inject_jsonc(original: &str, key: &str, tool_name: &str, tool_config: &serde_json::Value) -> Option<String> {"
replacement = """fn inject_jsonc(original: &str, key: &str, tool_name: &str, tool_config: &serde_json::Value) -> Option<String> {
    if original.trim().is_empty() || original.trim() == "{}" {
        let tool_config_str = serde_json::to_string_pretty(tool_config).unwrap().replace("\\n", "\\n      ");
        let injection = format!("\\"{}\\": {}", tool_name, tool_config_str);
        return Some(format!("{{\\n  \\"{}\\": {{\\n    {}\\n  }}\\n}}", key, injection.replace("      ", "    ")));
    }
"""

if target in content:
    content = content.replace(target, replacement)
    with open("src/install.rs", "w") as f:
        f.write(content)
    print("Patched empty handling")
else:
    print("Target not found")
