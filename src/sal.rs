//! `.sal` launch-file handling. A `.sal` is simple `KEY=VALUE` lines. We build the
//! hosted `.sal` from the SGE launch tokens: keep the real `KEY`, rewrite
//! `GAMEHOST`/`GAMEPORT` to point at the MUD Mobile router, and apply per-front-end
//! overrides. Mirrors `../mudmobile/web/src/lib/sal.ts` and
//! `../mudmobile/spike/hostedsal.mjs:108-117`.

use std::io::Write;
use std::path::PathBuf;

use crate::error::{AppError, AppResult};
use crate::model::SalOverrides;

/// True if `k` is a valid `.sal` key (`[A-Za-z0-9_]+`).
fn is_key(k: &str) -> bool {
    !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Parse a `.sal` file into ordered (UPPERCASED key, value) pairs. Lines that don't
/// match `KEY=VALUE` are skipped. Mirrors `parseSal` in `sal.ts`.
pub fn parse_sal(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        if let Some((k, v)) = line.split_once('=') {
            if is_key(k) {
                out.push((k.to_uppercase(), v.to_string()));
            }
        }
    }
    out
}

/// Build the hosted `.sal` contents from SGE launch tokens. Keeps every token
/// (original key case + order), rewrites `GAMEHOST`/`GAMEPORT` to the router, keeps
/// `KEY` unchanged, and applies `overrides` to GAME/GAMEFILE/FULLGAMENAME *when those
/// lines are present* (matching Lich's `.sub` semantics in `launch_data.rb`).
pub fn build_hosted_sal(
    tokens: &[(String, String)],
    host: &str,
    port: u16,
    overrides: &SalOverrides,
) -> String {
    let mut lines = Vec::with_capacity(tokens.len());
    for (k, v) in tokens {
        let value = match k.to_uppercase().as_str() {
            "GAMEHOST" => host.to_string(),
            "GAMEPORT" => port.to_string(),
            "KEY" => v.clone(), // real key, unchanged
            "GAME" => overrides.game.clone().unwrap_or_else(|| v.clone()),
            "GAMEFILE" => overrides.gamefile.clone().unwrap_or_else(|| v.clone()),
            "FULLGAMENAME" => overrides.fullgamename.clone().unwrap_or_else(|| v.clone()),
            _ => v.clone(),
        };
        lines.push(format!("{k}={value}"));
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// Write `.sal` contents to a temp file and return its path. The file is kept (not
/// auto-deleted) so the front end can open it.
pub fn write_temp_sal(contents: &str) -> AppResult<PathBuf> {
    let tmp = tempfile::Builder::new()
        .prefix("mudmobile")
        .suffix(".sal")
        .tempfile()
        .map_err(|e| AppError::Io(format!("creating temp .sal: {e}")))?;
    {
        let mut f = tmp.as_file();
        f.write_all(contents.as_bytes())
            .map_err(|e| AppError::Io(format!("writing .sal: {e}")))?;
        f.flush().map_err(|e| AppError::Io(e.to_string()))?;
    }
    let (_file, path) = tmp
        .keep()
        .map_err(|e| AppError::Io(format!("persisting .sal: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tokens() -> Vec<(String, String)> {
        // Shape of an `L\tOK\t...` STORM response (keys upper-cased as the server sends them).
        vec![
            ("GAMEHOST".into(), "storm.gs4.game.play.net".into()),
            ("GAMEPORT".into(), "10124".into()),
            ("KEY".into(), "abcdef0123456789abcdef0123456789".into()),
            ("GAMECODE".into(), "GS3".into()),
            ("GAME".into(), "STORM".into()),
            ("FULLGAMENAME".into(), "GemStone IV".into()),
            ("GAMEFILE".into(), "STORM.EXE".into()),
        ]
    }

    #[test]
    fn parse_sal_basic() {
        let text = "GAMEHOST=h\r\nGAMEPORT=7000\nkey=ABC\n\nnot a sal line\nGAME=STORM";
        let parsed = parse_sal(text);
        assert_eq!(
            parsed,
            vec![
                ("GAMEHOST".to_string(), "h".to_string()),
                ("GAMEPORT".to_string(), "7000".to_string()),
                ("KEY".to_string(), "ABC".to_string()),
                ("GAME".to_string(), "STORM".to_string()),
            ]
        );
    }

    #[test]
    fn build_rewrites_host_port_keeps_key() {
        let out = build_hosted_sal(
            &sample_tokens(),
            "play.mudmobile.com",
            7000,
            &SalOverrides::default(),
        );
        assert!(out.contains("GAMEHOST=play.mudmobile.com"));
        assert!(out.contains("GAMEPORT=7000"));
        // real key untouched
        assert!(out.contains("KEY=abcdef0123456789abcdef0123456789"));
        // other tokens preserved
        assert!(out.contains("GAMECODE=GS3"));
        assert!(out.contains("GAME=STORM"));
        assert!(out.ends_with('\n'));
        // original eaccess host must be gone
        assert!(!out.contains("play.net"));
    }

    #[test]
    fn build_preserves_order() {
        let out = build_hosted_sal(
            &sample_tokens(),
            "play.mudmobile.com",
            7000,
            &SalOverrides::default(),
        );
        let keys: Vec<&str> = out
            .lines()
            .filter_map(|l| l.split('=').next())
            .collect();
        assert_eq!(
            keys,
            vec!["GAMEHOST", "GAMEPORT", "KEY", "GAMECODE", "GAME", "FULLGAMENAME", "GAMEFILE"]
        );
    }

    #[test]
    fn build_applies_wizard_overrides() {
        let ov = SalOverrides {
            game: Some("WIZ".into()),
            gamefile: Some("WIZARD.EXE".into()),
            fullgamename: Some("Wizard Front End".into()),
        };
        let out = build_hosted_sal(&sample_tokens(), "play.mudmobile.com", 7000, &ov);
        assert!(out.contains("GAME=WIZ"));
        assert!(out.contains("GAMEFILE=WIZARD.EXE"));
        assert!(out.contains("FULLGAMENAME=Wizard Front End"));
        // key still preserved
        assert!(out.contains("KEY=abcdef0123456789abcdef0123456789"));
    }
}
