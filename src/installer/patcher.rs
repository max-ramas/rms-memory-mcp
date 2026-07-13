pub fn strip_json_comments(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut in_string = false;
    let mut in_comment = false;
    let mut in_multiline_comment = false;
    let mut chars = json.chars().peekable();

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next_c) = chars.next() {
                    out.push(next_c);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if in_comment {
            if c == '\n' {
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
            } else if c == '\n' {
                out.push('\n');
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

pub fn inject_jsonc(
    original: &str,
    key: &str,
    tool_name: &str,
    tool_config: &serde_json::Value,
) -> Option<String> {
    if original.trim().is_empty() || original.trim() == "{}" {
        let tool_config_str = serde_json::to_string_pretty(tool_config)
            .unwrap()
            .replace("\n", "\n      ");
        let injection = format!("\"{}\": {}", tool_name, tool_config_str);
        return Some(format!(
            "{{\n  \"{}\": {{\n    {}\n  }}\n}}",
            key,
            injection.replace("      ", "    ")
        ));
    }

    let stripped = strip_json_comments(original);
    let mut json = serde_json::from_str::<serde_json::Value>(&stripped).ok()?;

    let obj = json.as_object_mut()?;
    if let Some(mcp) = obj.get(key)
        && let Some(mcp_obj) = mcp.as_object()
        && mcp_obj.contains_key(tool_name)
    {
        // Already exists — replace the existing block in-place
        let entry_pattern = format!(r#""{}"\s*:\s*\{{[^{{}}]*\}}"#, regex::escape(tool_name));
        if let Ok(re) = regex::Regex::new(&entry_pattern)
            && let Some(mat) = re.find(original)
        {
            let before_match = &original[..mat.start()];
            let indent = before_match
                .rfind('\n')
                .map(|nl| {
                    let line_start = nl + 1;
                    let spaces: String = before_match[line_start..]
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect();
                    spaces
                })
                .unwrap_or_else(|| "    ".to_string());
            let inner_indent = format!("{}  ", indent);

            let new_config_str = serde_json::to_string_pretty(tool_config).unwrap();
            let new_config_indented = new_config_str.replace("\n", &format!("\n{}", inner_indent));
            let replacement = format!("\"{}\": {}", tool_name, new_config_indented);

            let mut patched = original.to_string();
            patched.replace_range(mat.range(), &replacement);
            return Some(patched);
        }
        return Some(original.to_string());
    }

    let tool_config_str = serde_json::to_string_pretty(tool_config).unwrap();
    let tool_config_str = tool_config_str.replace("\n", "\n      ");
    let injection = format!("\"{}\": {}", tool_name, tool_config_str);

    if obj.contains_key(key) {
        let pattern = format!(r#"("{}"\s*:\s*\{{)"#, key);
        let re = regex::Regex::new(&pattern).unwrap();
        if let Some(mat) = re.find(original) {
            let mut patched = original.to_string();
            let after_brace = &original[mat.end()..];
            let just_whitespace_then_close = after_brace.trim_start().starts_with("}");
            if just_whitespace_then_close {
                patched.insert_str(mat.end(), &format!("\n      {}\n    ", injection));
            } else {
                patched.insert_str(mat.end(), &format!("\n      {},", injection));
            }
            return Some(patched);
        }
    } else {
        if let Some(last_brace) = original.rfind('}') {
            let mut patched = original[..last_brace].to_string();
            let trimmed = patched.trim_end();
            patched.truncate(trimmed.len());
            let needs_comma = !patched.ends_with(',') && !patched.ends_with('{');
            if needs_comma {
                patched.push(',');
            }
            patched.push_str(&format!(
                "\n  \"{}\": {{\n    {}\n  }}\n}}",
                key,
                injection.replace("      ", "    ")
            ));
            return Some(patched);
        }
    }

    None
}

pub fn remove_key(original: &str, key: &str, tool_name: &str) -> Option<String> {
    let stripped = strip_json_comments(original);
    let json = serde_json::from_str::<serde_json::Value>(&stripped).ok()?;
    let obj = json.as_object()?;

    // Check if the key + tool_name exist
    let mcp = obj.get(key)?;
    let mcp_obj = mcp.as_object()?;
    if !mcp_obj.contains_key(tool_name) {
        return Some(original.to_string());
    }

    // Regex to find and remove the tool_name entry including trailing comma
    let entry_pattern = format!(
        r#""{}"\s*:\s*\{{[^{{}}]*\}}\s*,?\s*"#,
        regex::escape(tool_name)
    );
    let re = regex::Regex::new(&entry_pattern).ok()?;
    if let Some(mat) = re.find(original) {
        let mut patched = original.to_string();
        patched.replace_range(mat.range(), "");
        // Clean up empty objects
        let empty_obj = format!(r#""{}"\s*:\s*\{{[^{{}}]*\}}\s*"#, regex::escape(key));
        if let Ok(re) = regex::Regex::new(&empty_obj)
            && re.is_match(&patched)
        {
            // Check if removing the last entry in the parent object
            patched = re.replace(&patched, "").to_string();
        }
        return Some(patched);
    }

    None
}

pub fn inject_toml(
    original: &str,
    tool_name: &str,
    tool_config: &serde_json::Value,
) -> Option<String> {
    let section_header = format!("[mcp_servers.{}]", tool_name);

    if original.contains(&section_header) {
        return Some(original.to_string());
    }

    let command = tool_config.get("command")?.as_str()?;
    let args = tool_config
        .get("args")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let entry = format!(
        "\n[mcp_servers.{}]\ncommand = \"{}\"\nargs = [{}]\n",
        tool_name, command, args
    );

    let mut result = original.to_string();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&entry);
    Some(result)
}
