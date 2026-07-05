with open("src/install.rs", "r") as f:
    content = f.read()

old_loop = """    for (candidate, mut json, ide, original_content) in selected_targets {
        let mut patched = false;
        
        if let Some(obj) = json.as_object_mut() {
            // Ensure the key exists
            if !obj.contains_key(ide.key) {
                obj.insert(ide.key.to_string(), serde_json::json!({}));
            }
            
            if let Some(servers) = obj.get_mut(ide.key).and_then(|v| v.as_object_mut()) {
                servers.insert("rms-memory".to_string(), serde_json::json!({
                    "command": my_exe_str,
                    "args": ["serve"]
                }));
                patched = true;
            }
        }
        
        if patched {
            let out = serde_json::to_string_pretty(&json)?;
            if out == original_content {"""

new_loop = """    for (candidate, mut json, ide, original_content) in selected_targets {
        let config_payload = if ide.name == "OpenCode" {
            serde_json::json!({
                "enabled": true,
                "type": "local",
                "command": [my_exe_str.clone(), "serve"]
            })
        } else {
            serde_json::json!({
                "command": my_exe_str.clone(),
                "args": ["serve"]
            })
        };

        let patched_content = inject_jsonc(&original_content, ide.key, "rms-memory", &config_payload);
        
        if let Some(out) = patched_content {
            if out == original_content {"""

if old_loop in content:
    content = content.replace(old_loop, new_loop)
    with open("src/install.rs", "w") as f:
        f.write(content)
    print("Patched loop successfully")
else:
    print("Could not find old loop")
