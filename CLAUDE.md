# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A cross-platform desktop **launcher** that routes Simutronics play (GemStone IV / DragonRealms)
through [MUD Mobile](https://mudmobile.com)'s hosted-Lich service, then hands a `.sal` launch file
to a user-chosen front end (Wrayth/StormFront, Warlock, Genie, Mudlet, Wizard). Rust + egui/eframe
native GUI, single self-contained binary (download size is a priority).

## Commands

```sh
cargo run                       # debug build + run the GUI
cargo build --release           # size-optimized ~7 MB stripped binary
cargo test                      # all unit/integration tests; no network required
cargo test hash_password        # run a single test by name (substring match)
cargo test --lib config::       # run one module's tests
```

There is no separate lint config; use `cargo clippy` and `cargo fmt`.

Linux runtime needs OpenSSL (TLS) and a running Secret Service (gnome-keyring/KWallet) for the
keychain. `scripts/install-icon-linux.sh` installs the icon + `.desktop` entry so the dev build
shows its icon on Wayland (the embedded `with_icon` is ignored there; the window is matched to the
`.desktop` file by `app_id` = `com.mudmobile.connector`).

## Architecture

The whole app is one binary crate. The end-to-end launch flow spans several modules, so understand
the data path before editing any single one:

**token → characters → SGE login (local) → register session → wait for runner → write .sal → spawn FE**

- `app.rs` — egui `App`, a wizard **state machine** over `enum Screen` (NeedsToken, Characters,
  NewConnection, PickDiscovered, Password, Busy, Launched, Settings). `update()` must never block:
  it only drains `Event`s from the worker, mutates state, and renders. All brand theming
  (`apply_theme`, color consts) lives here.
- `worker.rs` — the single **background thread**. egui is immediate-mode, so every blocking
  operation (SGE handshake, HTTP, runner polling, FE spawn) runs here. UI sends `Command`, worker
  emits `Event` and calls `ctx.request_repaint()` to wake the UI. `launch()` is the core sequence;
  `wait_for_runner()` polls `GET /api/sessions/{id}` until `ready` (bounded by `RUNNER_WAIT_MAX`).
- `sge.rs` — Simutronics **SGE/EAccess** protocol, ported from Lich (`https://github.com/elanthia-online/lich-5`).
  Connects to `eaccess.play.net:7910` via `SgeStream` (enum: TLS or plaintext). TLS is the default
  (`Config.use_tls`). Cert acceptance: an accept-any handshake first checks the bundled
  `assets/simu.pem` pin (`cert_matches_pin`, today's self-signed case, one handshake); if it doesn't
  match, it re-handshakes with full system-CA + hostname verification (future-proofs a CA migration).
  Anything else is refused — no encrypted-but-unverified mode. On a TLS failure the worker emits
  `Event::TlsRetryOffer` and the UI offers a one-off **retry over plaintext** (re-issues with
  `use_tls=false`). Handshake K→A→M→F→G→P→C→L. Password hash is
  `(((pw[i]-32) ^ hashkey[i]) + 32) & 0xff` over latin1 bytes — the highest-value test.
- `mudmobile.rs` — HTTP API client (`ureq`). Bearer `wlk_` token. Maps HTTP status → typed
  `AppError` (401/402/409/400/502). `gamehost_allowed()` is an **anti-SSRF allowlist** — keep it
  tight.
- `sal.rs` — `.sal` parse/build. `build_hosted_sal()` keeps the real `KEY`, rewrites
  `GAMEHOST`/`GAMEPORT`, applies per-FE overrides.
- `frontends.rs` — default FE registry + spawn. `command_template` uses `%1` for the `.sal` path;
  `/`→`\` on Windows.
- `config.rs` / `keychain.rs` — TOML config (no secrets) / OS keychain (token + per-account
  passwords). `model.rs` / `error.rs` — shared types.
- `main.rs` — eframe entry; embeds the icon, sets the Wayland/X11 `app_id` and window title.

## Non-obvious constraints

**Security invariants (must always hold; tested):**

1. The play.net password and the raw game `KEY` **never leave the machine** — SGE runs locally.
2. Only `keyHash = sha256(key)` is sent to the HTTP API, never the raw key.
3. The `wlk_` token and per-account passwords live **only in the OS keychain**, never in config.
4. The raw key is transmitted only as the front end's first TCP line (via the `.sal`).
5. `gamehost` must pass the anti-SSRF allowlist before `POST /api/sessions`.
6. Scrub `KEY` from any logs.

**Other gotchas:**

- **TLS stack is `native-tls`, not rustls** — the build box has no cmake, so aws-lc-rs can't build.
  Don't reintroduce a rustls/aws-lc dependency. One TLS stack serves both HTTPS (ureq) and the SGE
  socket.
- **TOML field ordering**: in `model.rs`, scalar fields must precede nested tables/arrays within a
  struct (`Config.frontends` last; `FrontEnd` scalars before `paths`/`sal_overrides`). TOML forbids
  a value after a table in the same table, so reordering serde fields can break serialization.
- **Plaintext connection model**: the API returns `connect:{host, port:443, tls:true}`, but we use
  `connect.host` with the port **overridden to `ROUTER_PLAINTEXT_PORT` (7000)** and no TLS in the
  `.sal`. Use the returned host (it can change); never hardcode it.
- **Always log in with `STORM`** — the hosted Lich serves a Stormfront stream. The bundled Wizard FE
  uses the older WIZ protocol and may be incompatible (the UI warns).
- **Wayland decorations**: the explicit `winit` dep with `wayland-csd-adwaita` is load-bearing for
  native titlebars; eframe's default winit omits it.
