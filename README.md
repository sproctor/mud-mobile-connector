# MUD Mobile Connector

A small, cross-platform desktop launcher that routes Simutronics play (GemStone IV /
DragonRealms) through [MUD Mobile](https://mudmobile.com)'s hosted-Lich service, then
hands a `.sal` launch file to the front end of your choice (Wrayth/StormFront, Warlock,
Genie, Mudlet, Wizard, …).

The play.net **password and game key never leave your machine**: the SGE login runs
locally and MUD Mobile only ever receives `sha256(key)`.

## How it works

1. Paste your MUD Mobile **device token** (`wlk_…`) once — stored in your OS keychain.
2. `GET /api/characters` lists your saved characters (or use **New connection** to log in
   with account + password and discover them via SGE, saving them back).
3. Pick a character + a front end. The first time you launch an account you enter its
   play.net password; it's saved to the OS keychain and reused after that. If a stored
   password ever fails SGE, the bad one is forgotten and you're prompted to re-enter it.
4. The app runs the **SGE/EAccess** handshake locally (`eaccess.play.net:7910`, TLS) →
   `{gamehost, gameport, key}`.
5. `POST /api/sessions` with `keyHash = sha256(key)` → a router endpoint + session id.
6. The app **waits for the runner to become functional**, polling `GET /api/sessions/{id}`
   and reporting live status as it boots (bounded by a ~2-minute timeout, after which it
   launches anyway since the router holds the connection).
7. A `.sal` is written locally (real `KEY` kept; `GAMEHOST`/`GAMEPORT` rewritten to the
   router on the **plaintext port 7000**) and the chosen front end is launched against the
   ready runner.
8. Optionally, the session is ended (`DELETE /api/sessions/{id}`) when the front end exits.

See `../mudmobile/docs/warlock-integration.md` for the full API contract this implements.

## Build & run

```sh
cargo run               # debug
cargo build --release   # optimized, ~7 MB stripped binary at target/release/mudmobile-connector
cargo test              # 26 unit/integration tests, no network required
```

Linux runtime needs OpenSSL (for TLS) and, for the keychain, a running Secret Service
(gnome-keyring / KWallet). Windows/macOS use the OS-native TLS + credential stores.

## Configuration

Config lives at the per-OS path (Linux: `~/.config/mudmobile-connector/config.toml`)
and holds the **editable front-end list** plus preferences. Edit it in **Settings**, or by
hand. The device token and per-account play.net passwords are in the OS keychain, never in config.

Each front end has a `command_template` where `%1` is replaced with the `.sal` path, an
executable path per OS, an optional `.sal` field override (e.g. Wizard → `GAME=WIZ`), and
a `protocol` (`storm`/`wiz`).

> **Note:** the hosted Lich serves a **Stormfront** stream, so Stormfront-protocol front
> ends are expected. The bundled **Wizard** entry uses the older WIZ protocol and may not
> be compatible — the UI warns when it's selected.

## Design notes

- **Stack:** Rust + egui/eframe (native GUI, no webview/JRE → small self-contained binary).
- **Networking:** blocking `ureq` (HTTPS) + `native-tls` for both HTTPS and the SGE socket
  (single TLS stack, OS-native, no bundled crypto). The eaccess cert is self-signed, so it's
  accepted and **trust-on-first-use pinned** (stored under the data dir), mirroring Lich.
- **Concurrency:** all blocking work runs on one worker thread (`worker.rs`); the egui UI
  only drains events and renders. The worker calls `request_repaint()` to wake the UI.
- **Connection model:** plaintext-direct (the front end connects straight to the router on
  port 7000). A local TLS-proxy mode could be added later, isolated to `sal.rs`/`worker.rs`.

### Module map
`sge.rs` SGE/EAccess handshake · `mudmobile.rs` HTTP API client · `sal.rs` `.sal` build/parse ·
`frontends.rs` registry + spawn · `config.rs`/`keychain.rs` persistence · `worker.rs`
background thread · `app.rs` egui UI/state machine · `model.rs`/`error.rs` shared types.

## Manual end-to-end test

Unit/integration tests cover the protocol logic offline. To validate against the live
service you need a real play.net account and a `wlk_` token. The MUD Mobile spike is the
oracle:

```sh
SIMU_ACCOUNT=… SIMU_PASSWORD=… SIMU_CHARACTER=… WLK_TOKEN=wlk_… \
  node ../mudmobile/spike/hostedsal.mjs --game DR --out /tmp/ref.sal
```

Then run this app, launch the same character, and compare the generated `.sal` (its path is
shown on the Launched screen) against `/tmp/ref.sal` — `GAMEHOST`/`GAMEPORT` should point at
the router and `KEY` should match.

## Status / follow-ups

- Plaintext-only connection model (per design choice); TLS-proxy mode not yet implemented.
- SGE socket uses a single read per response (like Lich's `sysread`); add grace-period
  coalescing if a response ever arrives split across TCP segments.
