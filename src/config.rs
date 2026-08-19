use std::path::PathBuf;

pub fn dir() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        Some(PathBuf::from(x).join("dock"))
    } else if let Ok(x) = std::env::var("HOME") {
        Some(PathBuf::from(x).join(".config/dock"))
    } else {
        None
    }
}
