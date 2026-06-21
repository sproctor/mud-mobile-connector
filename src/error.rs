//! Unified application error surfaced to the UI. Domain modules (`sge`, `mudmobile`, ...)
//! return this directly; the variants map 1:1 onto the user-facing messages and the
//! MUD Mobile HTTP status codes documented in `warlock-integration.md` §4.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    /// HTTP 401 — token missing/invalid/revoked. UI should prompt to re-paste the token.
    #[error("Your MUD Mobile token is missing, invalid, or revoked. Re-paste it in Settings.")]
    TokenInvalid,

    /// HTTP 402 — no active subscription / beta access.
    #[error("A MUD Mobile subscription is required. Visit mudmobile.com.")]
    SubscriptionRequired,

    /// HTTP 409 — already at the concurrent-session cap.
    #[error("You're at your session limit ({active}/{limit}). End a session and retry.")]
    ConcurrentLimit { limit: u32, active: u32 },

    /// HTTP 400 — request failed validation (a client bug; log it).
    #[error("MUD Mobile rejected the request (invalid_body): {0}")]
    InvalidBody(String),

    /// HTTP 502 — cloud boot failed; transient, allow retry.
    #[error("MUD Mobile couldn't start a session (transient). Try again.")]
    MachineCreateFailed,

    /// Any other non-success HTTP response.
    #[error("MUD Mobile API error: {0}")]
    Api(String),

    /// SGE/EAccess authentication or protocol failure (e.g. bad password, failed sub check).
    #[error("SGE login failed: {0}")]
    SgeAuth(String),

    /// The selected character wasn't in the EAccess character list.
    #[error("Character {0:?} was not found on this account.")]
    CharacterNotFound(String),

    /// SGE returned a gamehost outside the *.play.net / *.simutronics.net allowlist.
    #[error("SGE returned an unexpected game host: {0:?}")]
    DisallowedGameHost(String),

    /// The configured front-end executable could not be found.
    #[error("Front end executable not found: {0}")]
    FrontEndNotFound(String),

    /// Socket / TLS / transport failure.
    #[error("Network error: {0}")]
    Network(String),

    /// Local I/O failure (writing the .sal, config, etc.).
    #[error("I/O error: {0}")]
    Io(String),

    /// Catch-all for unexpected conditions.
    #[error("{0}")]
    Other(String),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
