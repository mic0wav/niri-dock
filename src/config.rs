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

pub fn read_or_seed_default(filename: &str, default: &str) -> String {
    let Some(dir) = dir() else {
        log::warn!("Could not resolve config directory, using bundled default for {filename}.");
        return default.to_string();
    };

    let path = dir.join(filename);

    match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => {
            if let Err(e) =
                std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, default))
            {
                log::warn!(
                    "Failed to write default {filename} to {}: {e}",
                    path.display()
                );
            } else {
                log::info!("Wrote default {filename} to {}", path.display());
            }
            default.to_string()
        }
    }
}
