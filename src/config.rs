//! Persisted configuration (TOML) under the per-OS config dir. Stores the editable
//! front-end list and a few preferences. The `wlk_` token lives in the OS keychain
//! (see `keychain.rs`), never here; the play.net password is never persisted.

use std::fs;
use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::model::Config;

fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("com", "mudmobile", "connector")
}

/// Path to `config.toml` (e.g. `~/.config/mudmobile-connector/config.toml` on Linux).
pub fn config_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("config.toml"))
}

/// Load config. Missing file -> defaults (which seed the front-end list). A corrupt
/// file -> defaults plus a warning (never crash the launcher over a hand-edited file).
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str::<Config>(&text).unwrap_or_else(|e| {
            log::warn!("config at {} is invalid ({e}); using defaults", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

/// Load config, writing defaults to disk on first run so the user has something to edit.
pub fn load_or_init() -> Config {
    let exists = config_path().map(|p| p.exists()).unwrap_or(false);
    let cfg = load();
    if !exists {
        if let Err(e) = save(&cfg) {
            log::warn!("could not write initial config: {e}");
        }
    }
    cfg
}

/// Persist config as pretty TOML, creating the config dir if needed.
pub fn save(cfg: &Config) -> AppResult<()> {
    let path = config_path().ok_or_else(|| AppError::Io("no config directory available".into()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(cfg)
        .map_err(|e| AppError::Io(format!("serializing config: {e}")))?;
    fs::write(&path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(cfg.frontends.len(), back.frontends.len());
        assert!(back.delete_session_on_exit);
        // a front end with nested overrides survives the round trip
        let wiz = back.frontends.iter().find(|f| f.name == "Wizard").unwrap();
        assert_eq!(wiz.sal_overrides.game.as_deref(), Some("WIZ"));
        // the windows path on Wrayth survives
        let wrayth = back.frontends.iter().find(|f| f.name.contains("Wrayth")).unwrap();
        assert!(wrayth.paths.windows.is_some());
    }
}
