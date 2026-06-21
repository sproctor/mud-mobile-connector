//! Shared data types. Field renames match the MUD Mobile JSON contract
//! (`docs/warlock-integration.md` §4) and the persisted TOML config.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// MUD Mobile API shapes
// ---------------------------------------------------------------------------

/// A saved character profile returned by `GET /api/characters`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    /// play.net account name (never a password).
    pub account: String,
    /// EAccess game code: DR/DRX/DRF/DRT/GS3/GSX/GSF...
    pub game: String,
    #[serde(rename = "characterCode")]
    pub character_code: String,
    #[serde(rename = "characterName")]
    pub character_name: String,
    #[serde(rename = "lastUsedAt", default)]
    pub last_used_at: Option<String>,
}

/// Body for `POST /api/characters` (idempotent upsert). No secrets.
#[derive(Debug, Clone, Serialize)]
pub struct CharacterUpsert {
    pub account: String,
    pub game: String,
    #[serde(rename = "characterCode")]
    pub character_code: String,
    #[serde(rename = "characterName")]
    pub character_name: String,
}

/// Body for `POST /api/sessions`. Carries only `keyHash`, never the raw key.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRequest {
    pub game: String,
    pub character: String,
    pub gamehost: String,
    pub gameport: u16,
    #[serde(rename = "keyHash")]
    pub key_hash: String,
}

/// Response from `POST /api/sessions`.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub connect: Connect,
}

/// Where the front end should connect (the router endpoint).
#[derive(Debug, Clone, Deserialize)]
pub struct Connect {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
}

/// Response from `GET /api/sessions/{id}` — runner status while it boots.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionStatus {
    pub id: String,
    /// launching | active | ended | failed
    #[serde(default)]
    pub status: String,
    /// Human-readable detail, e.g. "Booting Lich…".
    #[serde(rename = "statusDetail", default)]
    pub status_detail: Option<String>,
    /// The runner's readiness callback fired (Lich is up and functional).
    #[serde(default)]
    pub ready: bool,
    #[serde(rename = "readyAt", default)]
    pub ready_at: Option<String>,
}

// ---------------------------------------------------------------------------
// SGE / EAccess results
// ---------------------------------------------------------------------------

/// One entry from the EAccess `C` character list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SgeChar {
    pub code: String,
    pub name: String,
}

/// Result of a full SGE login (`L ... STORM`).
#[derive(Debug, Clone)]
pub struct SgeResult {
    pub gamehost: String,
    pub gameport: u16,
    /// The real launch key. NEVER sent to the HTTP API; only `sha256(key)` is.
    pub key: String,
    /// All KEY=VALUE launch tokens from the `L` response, original case + order preserved.
    /// These become the `.sal` lines (after rewriting GAMEHOST/GAMEPORT).
    pub launch_tokens: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Front ends & config (persisted as TOML)
// ---------------------------------------------------------------------------

/// Wire protocol a front end speaks. Drives the `.sal` GAME field and the
/// Wizard-vs-Stormfront compatibility warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Storm,
    Wiz,
}

/// Per-OS executable path for a front end.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerOsPath {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub macos: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
}

impl PerOsPath {
    /// The path configured for the current OS, if any.
    pub fn for_current_os(&self) -> Option<&str> {
        if cfg!(target_os = "windows") {
            self.windows.as_deref()
        } else if cfg!(target_os = "macos") {
            self.macos.as_deref()
        } else {
            self.linux.as_deref()
        }
    }

    /// Set the path for the current OS (used by the Settings file picker).
    pub fn set_for_current_os(&mut self, value: Option<String>) {
        if cfg!(target_os = "windows") {
            self.windows = value;
        } else if cfg!(target_os = "macos") {
            self.macos = value;
        } else {
            self.linux = value;
        }
    }
}

/// `.sal` field overrides applied per front end (mirrors Lich's `launch_data.rb`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SalOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamefile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullgamename: Option<String>,
}

/// A launchable front end. `command_template` contains `%1` where the .sal path goes.
/// Field order matters for TOML: scalar fields precede the nested `paths` /
/// `sal_overrides` tables (TOML forbids a value after a table within the same table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontEnd {
    pub name: String,
    #[serde(default)]
    pub protocol: Protocol,
    /// e.g. `"%1"` (just the sal path) or `"-some-flag %1"`. `%1` -> the .sal path.
    pub command_template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub paths: PerOsPath,
    #[serde(default)]
    pub sal_overrides: SalOverrides,
}

fn default_true() -> bool {
    true
}

/// Persisted application configuration (TOML). Secrets are NOT stored here: the `wlk_`
/// token and per-account play.net passwords live in the OS keychain.
/// Field order matters for TOML: scalars first, the `frontends` array-of-tables last.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_character_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_frontend: Option<String>,
    #[serde(default = "default_true")]
    pub delete_session_on_exit: bool,
    /// Save play.net passwords to the OS keychain and reuse them.
    #[serde(default = "default_true")]
    pub remember_password: bool,
    /// Override the API base URL (for testing/staging). Defaults to https://mudmobile.com.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    #[serde(default)]
    pub frontends: Vec<FrontEnd>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            frontends: crate::frontends::default_frontends(),
            last_used_character_id: None,
            last_used_frontend: None,
            delete_session_on_exit: true,
            remember_password: true,
            api_base: None,
        }
    }
}
