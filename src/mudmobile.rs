//! MUD Mobile HTTP API client. Contract: `../mudmobile/docs/warlock-integration.md` §4.
//! Base `https://mudmobile.com`, JSON, `Authorization: Bearer wlk_…`.
//!
//! Uses native-tls (OS cert store) for HTTPS. The only secret that ever transits
//! this client is `keyHash = sha256(key)` — never the password, never the raw key.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::model::{Character, CharacterUpsert, SessionRequest, SessionResponse, SessionStatus};

const DEFAULT_BASE: &str = "https://mudmobile.com";

/// `sha256(key)` as 64 lowercase hex chars — the only key material sent to the API.
pub fn key_hash(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    hex::encode(h.finalize())
}

/// Anti-SSRF allowlist for the SGE-returned game host. Must mirror MUD Mobile's
/// server-side `isAllowedGameHost` (web/src/lib/session.ts) — keep the two in sync.
///
/// Simutronics now fronts game servers with AWS Elastic Load Balancers
/// (e.g. `nlb-hydra-<hash>.elb.<region>.amazonaws.com`), so AWS ELB endpoints are
/// allowed in addition to play.net / simutronics.net. The `.elb.` label requirement
/// keeps this to load balancers rather than arbitrary `*.amazonaws.com` services.
pub fn gamehost_allowed(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    h == "play.net"
        || h == "simutronics.net"
        || h.ends_with(".play.net")
        || h.ends_with(".simutronics.net")
        || (h.ends_with(".amazonaws.com") && h.contains(".elb."))
}

pub struct Api {
    agent: ureq::Agent,
    base: String,
    bearer: String,
}

impl Api {
    /// Build a client. `base` overrides the default API URL (for testing/staging).
    pub fn new(base: Option<&str>, token: &str) -> AppResult<Self> {
        let connector = native_tls::TlsConnector::new()
            .map_err(|e| AppError::Network(format!("TLS init: {e}")))?;
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .tls_connector(Arc::new(connector))
            .build();
        let base = base
            .unwrap_or(DEFAULT_BASE)
            .trim_end_matches('/')
            .to_string();
        Ok(Api {
            agent,
            base,
            bearer: format!("Bearer {token}"),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    // ---- characters -------------------------------------------------------

    /// `GET /api/characters` -> saved characters (most-recent first).
    pub fn list_characters(&self) -> AppResult<Vec<Character>> {
        let resp = handle(
            self.agent
                .get(&self.url("/api/characters"))
                .set("Authorization", &self.bearer)
                .call(),
        )?;
        let parsed: CharactersResp = into_json(resp)?;
        Ok(parsed.characters)
    }

    /// `POST /api/characters` -> upsert a discovered character (idempotent). No secrets.
    pub fn upsert_character(&self, c: &CharacterUpsert) -> AppResult<Character> {
        let resp = handle(
            self.agent
                .post(&self.url("/api/characters"))
                .set("Authorization", &self.bearer)
                .send_json(c),
        )?;
        let parsed: CharacterResp = into_json(resp)?;
        Ok(parsed.character)
    }

    /// `DELETE /api/characters/{id}`.
    pub fn delete_character(&self, id: &str) -> AppResult<()> {
        handle(
            self.agent
                .delete(&self.url(&format!("/api/characters/{id}")))
                .set("Authorization", &self.bearer)
                .call(),
        )?;
        Ok(())
    }

    // ---- sessions ---------------------------------------------------------

    /// `POST /api/sessions` -> boot a hosted session. Validates the gamehost allowlist
    /// client-side for a clearer error than the server's 400.
    pub fn create_session(&self, req: &SessionRequest) -> AppResult<SessionResponse> {
        if !gamehost_allowed(&req.gamehost) {
            return Err(AppError::DisallowedGameHost(req.gamehost.clone()));
        }
        let resp = handle(
            self.agent
                .post(&self.url("/api/sessions"))
                .set("Authorization", &self.bearer)
                .send_json(req),
        )?;
        into_json(resp)
    }

    /// `GET /api/sessions/{id}` -> runner status (used to wait for the runner to be ready).
    pub fn get_session(&self, id: &str) -> AppResult<SessionStatus> {
        let resp = handle(
            self.agent
                .get(&self.url(&format!("/api/sessions/{id}")))
                .set("Authorization", &self.bearer)
                .call(),
        )?;
        into_json(resp)
    }

    /// `DELETE /api/sessions/{id}` -> end a session / free the concurrency slot.
    pub fn delete_session(&self, id: &str) -> AppResult<()> {
        handle(
            self.agent
                .delete(&self.url(&format!("/api/sessions/{id}")))
                .set("Authorization", &self.bearer)
                .call(),
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Response wrappers + error mapping
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CharactersResp {
    #[serde(default)]
    characters: Vec<Character>,
}

#[derive(Deserialize)]
struct CharacterResp {
    character: Character,
}

fn into_json<T: serde::de::DeserializeOwned>(resp: ureq::Response) -> AppResult<T> {
    resp.into_json::<T>()
        .map_err(|e| AppError::Api(format!("parsing response: {e}")))
}

/// Turn a ureq result into our typed errors. Non-2xx -> `Error::Status`; map per §4.
fn handle(r: Result<ureq::Response, ureq::Error>) -> AppResult<ureq::Response> {
    match r {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            Err(map_status(code, &body))
        }
        Err(ureq::Error::Transport(t)) => Err(AppError::Network(t.to_string())),
    }
}

fn map_status(code: u16, body: &str) -> AppError {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    match code {
        401 => AppError::TokenInvalid,
        402 => AppError::SubscriptionRequired,
        409 => AppError::ConcurrentLimit {
            limit: find_u32(&v, "limit"),
            active: find_u32(&v, "active"),
        },
        400 => AppError::InvalidBody(detail_string(&v).unwrap_or_else(|| body.to_string())),
        502 => AppError::MachineCreateFailed,
        _ => AppError::Api(format!("HTTP {code}: {}", truncate(body, 200))),
    }
}

/// Find a u32 field at the top level or under `detail`.
fn find_u32(v: &serde_json::Value, key: &str) -> u32 {
    v.get(key)
        .or_else(|| v.get("detail").and_then(|d| d.get(key)))
        .and_then(|x| x.as_u64())
        .unwrap_or(0) as u32
}

fn detail_string(v: &serde_json::Value) -> Option<String> {
    match v.get("detail") {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
        None => v
            .get("error")
            .and_then(|e| e.as_str())
            .map(|s| s.to_string()),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use tiny_http::{Header, Response, Server};

    #[test]
    fn key_hash_known_vector() {
        assert_eq!(
            key_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(key_hash("x").len(), 64);
    }

    #[test]
    fn gamehost_allowlist() {
        assert!(gamehost_allowed("storm.gs4.game.play.net"));
        assert!(gamehost_allowed("dr.simutronics.net"));
        assert!(gamehost_allowed("play.net"));
        // AWS ELB endpoints (Simutronics' current infrastructure).
        assert!(gamehost_allowed(
            "nlb-hydra-21a184f44303ee1d.elb.us-east-2.amazonaws.com"
        ));
        assert!(!gamehost_allowed("evil.example.com"));
        assert!(!gamehost_allowed("notplay.net.evil.com"));
        // amazonaws.com but not an ELB -> rejected.
        assert!(!gamehost_allowed("evil-bucket.s3.amazonaws.com"));
        // not actually an amazonaws.com host -> rejected.
        assert!(!gamehost_allowed("elb.amazonaws.com.evil.com"));
    }

    struct Captured {
        method: String,
        auth: Option<String>,
        body: String,
    }

    /// Start a one-request stub server; returns its base URL and a channel that
    /// receives what the client actually sent.
    fn one_shot(status: u16, resp_body: &'static str) -> (String, mpsc::Receiver<Captured>) {
        let server = Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut req = server.recv().unwrap();
            let auth = req.headers().iter().find_map(|h| {
                if h.field.as_str().as_str().eq_ignore_ascii_case("authorization") {
                    Some(h.value.as_str().to_string())
                } else {
                    None
                }
            });
            let method = req.method().as_str().to_string();
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body).ok();
            tx.send(Captured { method, auth, body }).ok();
            let resp = Response::from_string(resp_body)
                .with_status_code(status)
                .with_header("Content-Type: application/json".parse::<Header>().unwrap());
            req.respond(resp).ok();
        });
        (format!("http://127.0.0.1:{port}"), rx)
    }

    #[test]
    fn list_characters_sends_bearer_and_parses() {
        let body = r#"{"characters":[{"id":"c1","account":"acct","game":"DR","characterCode":"C001","characterName":"Alice","lastUsedAt":null}]}"#;
        let (base, rx) = one_shot(200, body);
        let api = Api::new(Some(&base), "wlk_secrettoken").unwrap();
        let chars = api.list_characters().unwrap();
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].character_name, "Alice");

        let cap = rx.recv().unwrap();
        assert_eq!(cap.method, "GET");
        assert_eq!(cap.auth.as_deref(), Some("Bearer wlk_secrettoken"));
    }

    #[test]
    fn create_session_sends_keyhash_only_no_secrets() {
        let body = r#"{"sessionId":"s1","connect":{"host":"play.mudmobile.com","port":443,"tls":true}}"#;
        let (base, rx) = one_shot(200, body);
        let api = Api::new(Some(&base), "wlk_t").unwrap();
        let req = SessionRequest {
            game: "DR".into(),
            character: "Alice".into(),
            gamehost: "dr.simutronics.net".into(),
            gameport: 11024,
            key_hash: key_hash("THE-REAL-SECRET-KEY"),
        };
        let resp = api.create_session(&req).unwrap();
        assert_eq!(resp.session_id, "s1");
        assert_eq!(resp.connect.host, "play.mudmobile.com");

        let cap = rx.recv().unwrap();
        assert_eq!(cap.method, "POST");
        // security invariants: hash present, raw key & password absent
        assert!(cap.body.contains("keyHash"));
        assert!(cap.body.contains(&key_hash("THE-REAL-SECRET-KEY")));
        assert!(!cap.body.contains("THE-REAL-SECRET-KEY"));
    }

    #[test]
    fn create_session_rejects_bad_gamehost_before_request() {
        // No server needed: client-side validation should fire first.
        let api = Api::new(Some("http://127.0.0.1:1"), "wlk_t").unwrap();
        let req = SessionRequest {
            game: "DR".into(),
            character: "Alice".into(),
            gamehost: "evil.example.com".into(),
            gameport: 11024,
            key_hash: key_hash("k"),
        };
        assert!(matches!(
            api.create_session(&req),
            Err(AppError::DisallowedGameHost(_))
        ));
    }

    #[test]
    fn maps_concurrent_limit() {
        let (base, _rx) = one_shot(
            409,
            r#"{"error":"concurrent_limit_reached","limit":2,"active":2}"#,
        );
        let api = Api::new(Some(&base), "wlk_t").unwrap();
        match api.list_characters() {
            Err(AppError::ConcurrentLimit { limit, active }) => {
                assert_eq!((limit, active), (2, 2));
            }
            other => panic!("expected ConcurrentLimit, got {other:?}"),
        }
    }

    #[test]
    fn get_session_parses_status() {
        let body = r#"{"id":"s1","status":"launching","statusDetail":"Booting Lich…","ready":false,"readyAt":null,"game":"DR","character":"Alice","createdAt":"2026-06-19T00:00:00Z"}"#;
        let (base, _rx) = one_shot(200, body);
        let api = Api::new(Some(&base), "wlk_t").unwrap();
        let st = api.get_session("s1").unwrap();
        assert_eq!(st.status, "launching");
        assert_eq!(st.status_detail.as_deref(), Some("Booting Lich…"));
        assert!(!st.ready);
    }

    #[test]
    fn maps_unauthorized() {
        let (base, _rx) = one_shot(401, r#"{"error":"unauthorized"}"#);
        let api = Api::new(Some(&base), "wlk_bad").unwrap();
        assert!(matches!(api.list_characters(), Err(AppError::TokenInvalid)));
    }
}
