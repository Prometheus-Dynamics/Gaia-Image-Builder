use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn default_config_path() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../examples/default-workspace/configs/default.toml")
        .display()
        .to_string()
}

pub fn write_temp_config(contents: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("gaia-config-pipeline-{nonce}.toml"));
    std::fs::write(&path, contents).expect("temp config should be written");
    path
}

pub fn write_temp_config_at(path: &PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("temp config parent should be created");
    }
    std::fs::write(path, contents).expect("temp config should be written");
}
