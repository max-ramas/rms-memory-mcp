use serde_json::json;
fn main() {
    let j = json!({"method": "initialize", "params": {"rootUri": "file:///Users/ramas"}});
    println!("{}", j);
}
