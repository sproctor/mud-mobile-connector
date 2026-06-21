//! Simutronics SGE / EAccess login, ported from Lich
//! (`../lich-5/lib/common/authentication/eaccess.rb`) and the MUD Mobile spike
//! (`../mudmobile/spike/eaccess.mjs`).
//!
//! Runs ENTIRELY locally: the play.net password and the resulting game key never
//! leave this machine. Connect TLS to `eaccess.play.net:7910` (the cert is
//! self-signed, so we accept it and trust-on-first-use pin it like Lich does),
//! then drive the tab-delimited handshake:
//!
//! `K` (hash key) -> hash password -> `A` (auth) -> `M` -> `F`/`G`/`P` (game info)
//! -> `C` (character list) -> `L <code> STORM` (launch tokens).

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use native_tls::TlsStream;

use crate::error::{AppError, AppResult};
use crate::model::{SgeChar, SgeResult};

const HOST: &str = "eaccess.play.net";
const PORT: u16 = 7910;
const IO_TIMEOUT: Duration = Duration::from_secs(15);
const READ_BUF: usize = 8192;

/// Hash the password with the server-provided hash key.
/// `hashed[i] = ((pw[i] - 32) ^ hashkey[i]) + 32` (mod 256), bytes treated as latin1.
/// Verified against `eaccess.rb:93` and `eaccess.mjs:68`.
fn hash_password(pw: &[u8], hashkey: &[u8]) -> AppResult<Vec<u8>> {
    if pw.len() > hashkey.len() {
        return Err(AppError::SgeAuth(
            "password longer than the server hash key".into(),
        ));
    }
    Ok(pw
        .iter()
        .zip(hashkey.iter())
        .map(|(&p, &k)| (((p as i32 - 32) ^ k as i32) + 32) as u8)
        .collect())
}

/// Encode a string as latin1 bytes (each char's low byte). play.net account names
/// and passwords are effectively latin1.
fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u8).collect()
}

/// Decode bytes as latin1 for text-protocol parsing.
fn latin1_str(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Parse the `C` character-list response into (code, name) entries.
/// Strips the `C\t<n>\t<n>\t<n>\t<n>` header, then reads tab-separated code/name pairs.
fn parse_char_list(resp: &str) -> Vec<SgeChar> {
    let trimmed = resp.trim_end_matches(['\n', '\r', '\t', ' ']);
    let parts: Vec<&str> = trimmed.split('\t').collect();
    if parts.len() <= 5 || parts[0] != "C" {
        return Vec::new();
    }
    parts[5..]
        .chunks(2)
        .filter(|c| c.len() == 2 && !c[0].is_empty() && !c[1].is_empty())
        .map(|c| SgeChar {
            code: c[0].to_string(),
            name: c[1].to_string(),
        })
        .collect()
}

/// Parse the `L\tOK\t...` launch response into ordered KEY=VALUE tokens (original case).
fn parse_launch_tokens(resp: &str) -> Vec<(String, String)> {
    let body = resp
        .trim_start()
        .strip_prefix("L\tOK\t")
        .unwrap_or(resp.trim_start());
    body.trim()
        .split('\t')
        .filter_map(|t| t.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Find a token value by case-insensitive key.
fn token<'a>(tokens: &'a [(String, String)], key: &str) -> Option<&'a str> {
    tokens
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Build an `SgeResult` from parsed launch tokens.
fn result_from_tokens(tokens: Vec<(String, String)>) -> AppResult<SgeResult> {
    let gamehost = token(&tokens, "GAMEHOST")
        .ok_or_else(|| AppError::SgeAuth("launch response missing GAMEHOST".into()))?
        .to_string();
    let gameport = token(&tokens, "GAMEPORT")
        .and_then(|p| p.trim().parse::<u16>().ok())
        .ok_or_else(|| AppError::SgeAuth("launch response missing/invalid GAMEPORT".into()))?;
    let key = token(&tokens, "KEY")
        .ok_or_else(|| AppError::SgeAuth("launch response missing KEY".into()))?
        .to_string();
    Ok(SgeResult {
        gamehost,
        gameport,
        key,
        launch_tokens: tokens,
    })
}

// ---------------------------------------------------------------------------
// Connection + TOFU cert pinning
// ---------------------------------------------------------------------------

/// Where we store the trust-on-first-use copy of the eaccess cert.
fn pinned_cert_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "mudmobile", "connector")
        .map(|d| d.data_dir().join("eaccess-cert.der"))
}

/// Trust-on-first-use pin check, mirroring Lich's `verify_pem`: store the cert on
/// first connect, warn + update (never hard-fail) if it changes later.
fn tofu_check(stream: &TlsStream<TcpStream>) {
    let Ok(Some(cert)) = stream.peer_certificate() else {
        return;
    };
    let Ok(der) = cert.to_der() else { return };
    let Some(path) = pinned_cert_path() else { return };
    if path.exists() {
        match fs::read(&path) {
            Ok(stored) if stored == der => {}
            Ok(_) => {
                log::warn!("eaccess.play.net certificate changed since last connect; updating pin");
                let _ = fs::write(&path, &der);
            }
            Err(_) => {
                let _ = fs::write(&path, &der);
            }
        }
    } else {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, &der);
    }
}

/// Open a TLS connection to eaccess. The cert is self-signed (hence the danger
/// flags); we pin it TOFU instead.
fn connect() -> AppResult<TlsStream<TcpStream>> {
    let addr = (HOST, PORT)
        .to_socket_addrs()
        .map_err(|e| AppError::Network(format!("resolving {HOST}: {e}")))?
        .next()
        .ok_or_else(|| AppError::Network(format!("no address for {HOST}")))?;
    let tcp = TcpStream::connect_timeout(&addr, IO_TIMEOUT)
        .map_err(|e| AppError::Network(format!("connecting to eaccess: {e}")))?;
    tcp.set_read_timeout(Some(IO_TIMEOUT)).ok();
    tcp.set_write_timeout(Some(IO_TIMEOUT)).ok();

    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| AppError::Network(format!("building TLS connector: {e}")))?;
    let stream = connector
        .connect(HOST, tcp)
        .map_err(|e| AppError::Network(format!("TLS handshake to eaccess failed: {e}")))?;
    tofu_check(&stream);
    Ok(stream)
}

/// Thin protocol wrapper over a stream (a TLS socket in production, a scripted mock
/// in tests — hence the generic `S`).
struct Sge<S> {
    stream: S,
}

impl Sge<TlsStream<TcpStream>> {
    /// Open a TLS connection to eaccess.
    fn open() -> AppResult<Self> {
        Ok(Sge { stream: connect()? })
    }
}

impl<S: Read + Write> Sge<S> {
    /// Send a raw command (bytes are written verbatim; callers include the trailing `\n`).
    fn send(&mut self, bytes: &[u8]) -> AppResult<()> {
        self.stream
            .write_all(bytes)
            .map_err(|e| AppError::Network(format!("writing to eaccess: {e}")))?;
        self.stream
            .flush()
            .map_err(|e| AppError::Network(format!("flushing to eaccess: {e}")))
    }

    /// Read one response burst (single read up to 8 KiB, like Lich's `sysread`).
    fn read_bytes(&mut self) -> AppResult<Vec<u8>> {
        let mut buf = vec![0u8; READ_BUF];
        let n = self
            .stream
            .read(&mut buf)
            .map_err(|e| AppError::Network(format!("reading from eaccess: {e}")))?;
        if n == 0 {
            return Err(AppError::Network("eaccess closed the connection".into()));
        }
        buf.truncate(n);
        Ok(buf)
    }

    /// Read one response decoded as latin1 text.
    fn read_str(&mut self) -> AppResult<String> {
        Ok(latin1_str(&self.read_bytes()?))
    }

    /// Authenticate and walk the handshake up to and including the `C` character list.
    /// Leaves the connection open, ready for an `L` launch.
    fn authenticate(&mut self, account: &str, password: &str, game_code: &str) -> AppResult<Vec<SgeChar>> {
        // K: request the password hash key (raw 32 bytes).
        self.send(b"K\n")?;
        let hashkey = self.read_bytes()?;

        // A: authenticate with the hashed password (raw bytes after the tab).
        let hashed = hash_password(&latin1_bytes(password), &hashkey)?;
        let mut a = Vec::new();
        a.extend_from_slice(b"A\t");
        a.extend_from_slice(&latin1_bytes(account));
        a.push(b'\t');
        a.extend_from_slice(&hashed);
        a.push(b'\n');
        self.send(&a)?;
        let resp = self.read_str()?;
        if !resp.contains("KEY\t") {
            let code = resp.split_whitespace().last().unwrap_or("").to_string();
            return Err(AppError::SgeAuth(format!(
                "{} (check account/password; try an UPPERCASE account name)",
                if code.is_empty() { resp.trim().to_string() } else { code }
            )));
        }

        // M: game menu.
        self.send(b"M\n")?;
        let m = self.read_str()?;
        if !m.starts_with("M\t") {
            return Err(AppError::SgeAuth(format!("unexpected M response: {}", m.trim())));
        }

        // F: subscription check.
        self.send(format!("F\t{game_code}\n").as_bytes())?;
        let f = self.read_str()?;
        if !["NORMAL", "PREMIUM", "TRIAL", "INTERNAL", "FREE"]
            .iter()
            .any(|s| f.contains(s))
        {
            return Err(AppError::SgeAuth(format!(
                "subscription check failed for {game_code}: {}",
                f.trim()
            )));
        }

        // G, P: game info (responses ignored, like Lich).
        self.send(format!("G\t{game_code}\n").as_bytes())?;
        self.read_str()?;
        self.send(format!("P\t{game_code}\n").as_bytes())?;
        self.read_str()?;

        // C: character list.
        self.send(b"C\n")?;
        let c = self.read_str()?;
        Ok(parse_char_list(&c))
    }

    /// Issue the `L <code> STORM` launch and parse the result.
    fn launch_char(&mut self, char_code: &str) -> AppResult<SgeResult> {
        self.send(format!("L\t{char_code}\tSTORM\n").as_bytes())?;
        let l = self.read_str()?;
        if !l.starts_with("L\tOK") {
            return Err(AppError::SgeAuth(format!("launch failed: {}", l.trim())));
        }
        result_from_tokens(parse_launch_tokens(&l))
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Log in and return the account's character list (for the "new connection" /
/// discovery flow). Does not launch.
pub fn discover_characters(account: &str, password: &str, game_code: &str) -> AppResult<Vec<SgeChar>> {
    let mut sge = Sge::open()?;
    sge.authenticate(account, password, game_code)
}

/// Full login for a known character code: authenticate, then `L ... STORM`.
/// Returns the launch tokens (incl. the real key) for building the `.sal`.
pub fn launch(account: &str, password: &str, game_code: &str, char_code: &str) -> AppResult<SgeResult> {
    let mut sge = Sge::open()?;
    let chars = sge.authenticate(account, password, game_code)?;
    if !chars.is_empty() && !chars.iter().any(|c| c.code == char_code) {
        return Err(AppError::CharacterNotFound(format!(
            "code {char_code} (have: {})",
            chars
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    sge.launch_char(char_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_password_known_vector() {
        // ((65-32)^16)+32 = 81 ; ((66-32)^32)+32 = 34
        assert_eq!(hash_password(b"AB", &[16, 32]).unwrap(), vec![81, 34]);
    }

    #[test]
    fn hash_password_identity_with_zero_key_and_high_byte() {
        // hashkey byte 0 => ((b-32)^0)+32 == b ; check a latin1 high byte (0xC1 = 193)
        assert_eq!(hash_password(&[193], &[0]).unwrap(), vec![193]);
    }

    #[test]
    fn hash_password_rejects_overlong_password() {
        assert!(hash_password(b"ABC", &[1, 2]).is_err());
    }

    #[test]
    fn parse_char_list_pairs() {
        let resp = "C\t1\t1\t10\t2\tC001\tAlice\tC002\tBob\n";
        assert_eq!(
            parse_char_list(resp),
            vec![
                SgeChar { code: "C001".into(), name: "Alice".into() },
                SgeChar { code: "C002".into(), name: "Bob".into() },
            ]
        );
    }

    #[test]
    fn parse_char_list_empty() {
        assert!(parse_char_list("C\t0\t0\t10\t0\n").is_empty());
    }

    #[test]
    fn parse_launch_tokens_and_result() {
        let l = "L\tOK\tUPPORT=5535\tGAME=STORM\tGAMECODE=GS3\tFULLGAMENAME=GemStone IV\tGAMEFILE=STORM.EXE\tGAMEHOST=storm.gs4.game.play.net\tGAMEPORT=10124\tKEY=deadbeef0123456789deadbeef012345\n";
        let tokens = parse_launch_tokens(l);
        assert_eq!(token(&tokens, "GAMEHOST"), Some("storm.gs4.game.play.net"));
        let r = result_from_tokens(tokens).unwrap();
        assert_eq!(r.gamehost, "storm.gs4.game.play.net");
        assert_eq!(r.gameport, 10124);
        assert_eq!(r.key, "deadbeef0123456789deadbeef012345");
        // GAME token preserved for later .sal building
        assert_eq!(token(&r.launch_tokens, "GAME"), Some("STORM"));
    }

    #[test]
    fn result_requires_key() {
        let tokens = parse_launch_tokens("L\tOK\tGAMEHOST=h.play.net\tGAMEPORT=10\n");
        assert!(result_from_tokens(tokens).is_err());
    }

    // --- handshake sequencing against a scripted mock stream ---------------

    use std::collections::VecDeque;
    use std::io;

    /// Returns canned responses (one per `read`), sinks writes for inspection.
    struct MockStream {
        responses: VecDeque<Vec<u8>>,
        written: Vec<u8>,
    }
    impl MockStream {
        fn new(responses: Vec<Vec<u8>>) -> Self {
            MockStream {
                responses: responses.into(),
                written: Vec::new(),
            }
        }
    }
    impl io::Read for MockStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.responses.pop_front() {
                Some(data) => {
                    let n = data.len().min(buf.len());
                    buf[..n].copy_from_slice(&data[..n]);
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }
    impl io::Write for MockStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn authenticate_walks_handshake_and_lists_chars() {
        let responses = vec![
            vec![0u8; 32],                                        // K -> 32-byte hash key
            b"A\t...\tKEY\tabcdef\t\n".to_vec(),                  // A -> contains KEY\t
            b"M\tDR\tDragonRealms\n".to_vec(),                    // M
            b"NORMAL\n".to_vec(),                                 // F -> subscription
            b"G\n".to_vec(),                                      // G (ignored)
            b"P\n".to_vec(),                                      // P (ignored)
            b"C\t1\t1\t10\t2\tC001\tAlice\tC002\tBob\n".to_vec(), // C -> character list
        ];
        let mut sge = Sge {
            stream: MockStream::new(responses),
        };
        let chars = sge.authenticate("acct", "pw", "DR").unwrap();
        assert_eq!(
            chars,
            vec![
                SgeChar { code: "C001".into(), name: "Alice".into() },
                SgeChar { code: "C002".into(), name: "Bob".into() },
            ]
        );
        // First command on the wire is K.
        assert!(sge.stream.written.starts_with(b"K\n"));
        // We sent F/G/P with the game code and finished with C.
        assert!(sge.stream.written.windows(4).any(|w| w == b"F\tDR"));
        assert!(sge.stream.written.ends_with(b"C\n"));
    }

    #[test]
    fn authenticate_rejects_bad_password() {
        let responses = vec![
            vec![0u8; 32],
            b"PROBLEM\tBADPASSWORD\n".to_vec(), // A response without KEY\t
        ];
        let mut sge = Sge {
            stream: MockStream::new(responses),
        };
        assert!(matches!(
            sge.authenticate("acct", "pw", "DR"),
            Err(AppError::SgeAuth(_))
        ));
    }

    #[test]
    fn launch_char_parses_ok_response() {
        let responses = vec![b"L\tOK\tGAMEHOST=storm.gs4.game.play.net\tGAMEPORT=10124\tKEY=deadbeef0123456789deadbeef012345\tGAME=STORM\n".to_vec()];
        let mut sge = Sge {
            stream: MockStream::new(responses),
        };
        let r = sge.launch_char("C001").unwrap();
        assert_eq!(r.gamehost, "storm.gs4.game.play.net");
        assert_eq!(r.gameport, 10124);
        assert_eq!(r.key, "deadbeef0123456789deadbeef012345");
        assert!(sge.stream.written.starts_with(b"L\tC001\tSTORM\n"));
    }
}
