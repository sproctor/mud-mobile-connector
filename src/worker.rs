//! Background worker. egui is immediate-mode and must never block, so all SGE +
//! HTTP work runs on one worker thread. The UI sends `Command`s; the worker emits
//! `Event`s and calls `request_repaint()` so the UI wakes immediately.
//!
//! The connection model is plaintext-direct (fire-and-forget): the `.sal` points the
//! front end at the router on the plaintext port (7000); we don't proxy.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::error::AppError;
use crate::model::{Character, CharacterUpsert, FrontEnd, SessionRequest, SgeChar};
use crate::mudmobile::{key_hash, Api};
use crate::{keychain, sal, sge};

/// The router's plaintext port (chosen connection model). We keep the router *host*
/// from the API response but use this port + no TLS for the front end's connection.
const ROUTER_PLAINTEXT_PORT: u16 = 7000;

/// Work requested by the UI. `base` is the optional API base override (else mudmobile.com).
/// `Clone` so the UI can stash a copy to re-issue (e.g. an unverified-TLS retry).
#[derive(Clone)]
pub enum Command {
    /// Validate a token and load the character list in one shot.
    LoadCharacters { base: Option<String>, token: String },
    /// Discover characters for an account via local SGE (the "new connection" flow),
    /// optionally upserting them into the MUD Mobile list.
    Discover {
        base: Option<String>,
        token: String,
        account: String,
        password: String,
        game: String,
        save: bool,
        /// Whether to persist the password to the keychain on success.
        remember: bool,
        /// Whether to use TLS (always cert-pinned) for the eaccess login, vs plaintext.
        use_tls: bool,
    },
    /// Full launch: SGE login -> create session -> write .sal -> spawn the front end.
    Launch {
        base: Option<String>,
        token: String,
        character: Character,
        /// Explicit password, or None to use the stored one (asking the UI if there isn't one).
        password: Option<String>,
        frontend: FrontEnd,
        /// Whether to persist the password to the keychain on success.
        remember: bool,
        /// Whether to use TLS (always cert-pinned) for the eaccess login, vs plaintext.
        use_tls: bool,
    },
}

/// Results pushed back to the UI.
pub enum Event {
    /// Progress message for the busy spinner.
    Stage(String),
    /// Character list loaded (token validated).
    Characters(Vec<Character>),
    /// Characters discovered via SGE for `account`/`game`.
    Discovered {
        account: String,
        game: String,
        chars: Vec<SgeChar>,
    },
    /// Front end launched successfully.
    Launched {
        session_id: String,
        sal_path: PathBuf,
        frontend: String,
    },
    /// No stored password for the account — the UI should prompt for one.
    NeedPassword,
    /// SGE rejected the password; the stored one (if any) was cleared. Prompt to re-enter.
    SgeAuthFailed { account: String, message: String },
    /// The verified TLS connection to eaccess failed. The UI offers to retry unverified.
    TlsRetryOffer { message: String },
    /// An operation failed (message is the user-facing AppError text).
    Failed { message: String, token_invalid: bool },
}

/// Spawn the worker thread. Returns the command sender and event receiver.
pub fn spawn(ctx: egui::Context) -> (Sender<Command>, Receiver<Event>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
    let (evt_tx, evt_rx) = mpsc::channel::<Event>();
    thread::Builder::new()
        .name("mm-worker".into())
        .spawn(move || {
            // Iterates until the UI drops its command sender (app exit).
            for cmd in cmd_rx {
                handle(cmd, &evt_tx, &ctx);
            }
        })
        .expect("spawn worker thread");
    (cmd_tx, evt_rx)
}

/// Send an event and wake the UI.
fn emit(tx: &Sender<Event>, ctx: &egui::Context, evt: Event) {
    let _ = tx.send(evt);
    ctx.request_repaint();
}

fn stage(tx: &Sender<Event>, ctx: &egui::Context, msg: &str) {
    emit(tx, ctx, Event::Stage(msg.to_string()));
}

fn handle(cmd: Command, tx: &Sender<Event>, ctx: &egui::Context) {
    let result = match cmd {
        Command::LoadCharacters { base, token } => load_characters(base, token, tx, ctx),
        Command::Discover {
            base,
            token,
            account,
            password,
            game,
            save,
            remember,
            use_tls,
        } => discover(
            base, token, account, password, game, save, remember, use_tls, tx, ctx,
        ),
        Command::Launch {
            base,
            token,
            character,
            password,
            frontend,
            remember,
            use_tls,
        } => launch(
            base, token, character, password, frontend, remember, use_tls, tx, ctx,
        ),
    };
    if let Err(e) = result {
        let token_invalid = matches!(e, AppError::TokenInvalid);
        emit(
            tx,
            ctx,
            Event::Failed {
                message: e.to_string(),
                token_invalid,
            },
        );
    }
}

fn load_characters(
    base: Option<String>,
    token: String,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) -> Result<(), AppError> {
    stage(tx, ctx, "Loading characters…");
    let api = Api::new(base.as_deref(), &token)?;
    let chars = api.list_characters()?;
    emit(tx, ctx, Event::Characters(chars));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn discover(
    base: Option<String>,
    token: String,
    account: String,
    password: String,
    game: String,
    save: bool,
    remember: bool,
    use_tls: bool,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) -> Result<(), AppError> {
    stage(tx, ctx, "Logging in to discover characters…");
    let chars = match sge::discover_characters(&account, &password, &game, use_tls) {
        Ok(c) => c,
        Err(AppError::SgeTls(message)) => {
            emit(tx, ctx, Event::TlsRetryOffer { message });
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    // The password worked — remember it if the user opted in.
    if remember {
        let _ = keychain::set_password(&account, &password);
    }
    if save && !chars.is_empty() {
        stage(tx, ctx, "Saving characters to MUD Mobile…");
        let api = Api::new(base.as_deref(), &token)?;
        for c in &chars {
            let _ = api.upsert_character(&CharacterUpsert {
                account: account.clone(),
                game: game.clone(),
                character_code: c.code.clone(),
                character_name: c.name.clone(),
            });
        }
    }
    emit(tx, ctx, Event::Discovered { account, game, chars });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn launch(
    base: Option<String>,
    token: String,
    character: Character,
    password: Option<String>,
    frontend: FrontEnd,
    remember: bool,
    use_tls: bool,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) -> Result<(), AppError> {
    // 0. Resolve the password: explicit > stored. If neither, ask the UI for one.
    let password = match password.or_else(|| keychain::get_password(&character.account)) {
        Some(p) => p,
        None => {
            emit(tx, ctx, Event::NeedPassword);
            return Ok(());
        }
    };

    // 1. Local SGE login — password & key stay on this machine.
    stage(tx, ctx, "Logging in via SGE…");
    let sge_result = match sge::launch(
        &character.account,
        &password,
        &character.game,
        &character.character_code,
        use_tls,
    ) {
        Ok(r) => r,
        Err(AppError::SgeAuth(message)) => {
            // Wrong password: forget it and ask the user to re-enter.
            let _ = keychain::delete_password(&character.account);
            emit(
                tx,
                ctx,
                Event::SgeAuthFailed {
                    account: character.account.clone(),
                    message,
                },
            );
            return Ok(());
        }
        Err(AppError::SgeTls(message)) => {
            emit(tx, ctx, Event::TlsRetryOffer { message });
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    // The password worked — remember it if the user opted in.
    if remember {
        let _ = keychain::set_password(&character.account, &password);
    }

    // 2. Register the session (only sha256(key) leaves the machine).
    stage(tx, ctx, "Registering hosted session…");
    let api = Api::new(base.as_deref(), &token)?;
    let session = api.create_session(&SessionRequest {
        game: character.game.clone(),
        character: character.character_name.clone(),
        gamehost: sge_result.gamehost.clone(),
        gameport: sge_result.gameport,
        key_hash: key_hash(&sge_result.key),
    })?;

    // 3. Wait for the runner to become functional, reporting status as it boots, so the
    //    front end connects to a ready runner instead of a cold-booting one.
    wait_for_runner(&api, &session.session_id, tx, ctx)?;

    // 4. Build the .sal pointing at the router (plaintext port), keeping the real key.
    stage(tx, ctx, "Writing launch file…");
    let sal_text = sal::build_hosted_sal(
        &sge_result.launch_tokens,
        &session.connect.host,
        ROUTER_PLAINTEXT_PORT,
        &frontend.sal_overrides,
    );
    let sal_path = sal::write_temp_sal(&sal_text)?;

    // 5. Spawn the front end and forget it: the launcher is fire-and-forget, and the
    //    hosted runner stays up idle after the front end exits — MUD Mobile auto-routes
    //    the next connection back to that same idle runner.
    stage(tx, ctx, &format!("Launching {}…", frontend.name));
    frontends::spawn(&frontend, &sal_path)?;

    emit(
        tx,
        ctx,
        Event::Launched {
            session_id: session.session_id,
            sal_path,
            frontend: frontend.name,
        },
    );
    Ok(())
}

use crate::frontends;

/// How long the runner-status loop is allowed to wait/poll before launching anyway.
const RUNNER_WAIT_MAX: Duration = Duration::from_secs(120);
const RUNNER_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Poll `GET /api/sessions/{id}` until the runner reports ready, reporting status as it
/// boots. Bounded by [`RUNNER_WAIT_MAX`] so a missing readiness callback can't hang
/// forever — past the cap we launch anyway (the router holds the connection through boot).
fn wait_for_runner(
    api: &Api,
    session_id: &str,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) -> Result<(), AppError> {
    let start = Instant::now();
    stage(tx, ctx, "Booting hosted runner…");
    loop {
        match api.get_session(session_id) {
            Ok(st) => {
                let detail = st
                    .status_detail
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| st.status.clone());
                match st.status.as_str() {
                    "failed" => {
                        return Err(AppError::Other(format!("Runner failed to start: {detail}")))
                    }
                    "ended" => {
                        return Err(AppError::Other(
                            "Session ended before the runner was ready.".into(),
                        ))
                    }
                    _ => {}
                }
                if st.ready {
                    stage(tx, ctx, "Runner ready.");
                    return Ok(());
                }
                let secs = start.elapsed().as_secs();
                stage(tx, ctx, &format!("Waiting for runner — {detail} ({secs}s)"));
            }
            // A bad token is fatal; anything else is treated as a transient status-check
            // blip and we keep waiting until the cap.
            Err(AppError::TokenInvalid) => return Err(AppError::TokenInvalid),
            Err(e) => {
                let secs = start.elapsed().as_secs();
                stage(tx, ctx, &format!("Waiting for runner — rechecking status ({secs}s): {e}"));
            }
        }
        if start.elapsed() >= RUNNER_WAIT_MAX {
            stage(
                tx,
                ctx,
                "Runner still starting — launching anyway; the router will hold the connection.",
            );
            return Ok(());
        }
        thread::sleep(RUNNER_POLL_INTERVAL);
    }
}
