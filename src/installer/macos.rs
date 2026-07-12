#[cfg(target_os = "macos")]
pub fn apply_entitlements(my_exe_str: &str) {
    println!(
        "[🔒] Applying macOS entitlements to bypass Library Validation (prevents crashes in sandboxed IDEs)..."
    );
    let entitlements = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.disable-library-validation</key>
    <true/>
</dict>
</plist>"#;
    let temp_dir = std::env::temp_dir();
    let entitlements_path = temp_dir.join("rms_entitlements.plist");
    let tmp_exe_path = temp_dir.join(format!("rms-memory-tmp-{}", std::process::id()));

    if std::fs::write(&entitlements_path, entitlements).is_ok() {
        // Copy the executable to a temporary location to sign it
        // Modifying a running executable in-place causes macOS to send SIGKILL.
        if let Err(e) = std::fs::copy(my_exe_str, &tmp_exe_path) {
            eprintln!("[⚠️] Failed to copy executable for signing: {}", e);
            let _ = std::fs::remove_file(&entitlements_path);
            return;
        }

        let status = std::process::Command::new("codesign")
            .args([
                "-s",
                "-",
                "-f",
                "--entitlements",
                entitlements_path.to_str().unwrap_or(""),
                tmp_exe_path.to_str().unwrap_or(""),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                // Atomically replace the running executable with the signed copy
                if let Err(e) = std::fs::rename(&tmp_exe_path, my_exe_str) {
                    eprintln!(
                        "[⚠️] Failed to replace executable with signed version: {}",
                        e
                    );
                    // Fallback: try copy if rename fails across filesystems
                    let _ = std::fs::copy(&tmp_exe_path, my_exe_str);
                } else {
                    println!("[✅] Successfully signed executable with entitlements.")
                }
            }
            _ => eprintln!(
                "[⚠️] Failed to sign executable. You may experience crashes in Claude Desktop. Try running: codesign -s - -f --entitlements path/to/entitlements.plist {}",
                my_exe_str
            ),
        }
        let _ = std::fs::remove_file(entitlements_path);
        let _ = std::fs::remove_file(tmp_exe_path);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_entitlements(_my_exe_str: &str) {
    // No action needed for non-macOS platforms
}
