//! Stdio MCP smoke: initialize + tools/list against the real `rms-memory` binary.
//!
//! Line-delimited JSON-RPC (no Content-Length framing). Runs unbound so no registry
//! is required for these two methods.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn read_rpc(reader: &mut impl BufRead) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read MCP stdout line");
    assert!(
        !line.trim().is_empty(),
        "MCP closed stdout before responding"
    );
    serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("invalid JSON from MCP: {e}: {line}"))
}

#[test]
fn mcp_stdio_initialize_and_tools_list() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rms-memory"))
        .args(["serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("RUST_LOG", "error")
        .spawn()
        .expect("spawn rms-memory serve");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "rms-smoke", "version": "0.0.0" }
            }
        })
    )
    .unwrap();
    stdin.flush().unwrap();

    let init = read_rpc(&mut reader);
    assert_eq!(init["id"], 1);
    assert!(init.get("error").is_none(), "initialize error: {init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "rms-memory", "{init}");
    assert!(
        init["result"]["serverInfo"]["version"].as_str().is_some(),
        "{init}"
    );

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        })
    )
    .unwrap();
    stdin.flush().unwrap();

    let listed = read_rpc(&mut reader);
    assert_eq!(listed["id"], 2);
    assert!(listed.get("error").is_none(), "tools/list error: {listed}");
    let tools = listed["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for required in [
        "rms_search",
        "rms_code_search",
        "rms_read",
        "rms_write",
        "rms_projects",
        "rms_overview",
        "rms_prune",
        "rms_checkpoint_save",
        "rms_system_instructions",
    ] {
        assert!(
            names.contains(&required),
            "missing tool {required}; have {names:?}"
        );
    }

    drop(stdin);
    let _ = child.wait_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
}

trait WaitTimeout {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl WaitTimeout for std::process::Child {
    fn wait_timeout(&mut self, dur: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None if start.elapsed() >= dur => return Ok(None),
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}
