//! The egui application: a small wizard state machine driving the launch flow, plus
//! a Settings screen for the token and the editable front-end list. All blocking work
//! is delegated to the worker thread (`worker.rs`); `update()` only drains events,
//! mutates state, and renders.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use eframe::egui;

use crate::model::{Character, Config, FrontEnd, PerOsPath, Protocol, SalOverrides, SgeChar};
use crate::worker::{self, Command, Event};
use crate::{config, keychain};

// MUD Mobile brand palette — exact values from ../mudmobile/web/src/app/globals.css.
const BG: egui::Color32 = egui::Color32::from_rgb(0x0a, 0x0a, 0x0a); // --bg
const FG: egui::Color32 = egui::Color32::from_rgb(0xed, 0xed, 0xed); // --fg
const BRAND: egui::Color32 = egui::Color32::from_rgb(0x34, 0xd3, 0x99); // --brand (spectral green)
const BRAND_DEEP: egui::Color32 = egui::Color32::from_rgb(0x10, 0xb9, 0x81); // --brand-deep
const MUTED: egui::Color32 = egui::Color32::from_rgb(0x9a, 0xa3, 0x9e); // --muted
const LINE: egui::Color32 = egui::Color32::from_rgb(0x18, 0x21, 0x1d); // --line
const SURFACE: egui::Color32 = egui::Color32::from_rgb(0x12, 0x16, 0x14); // button/input fill over black
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(0x1b, 0x24, 0x1f);

/// Apply the MUD Mobile black + spectral-green theme.
fn apply_theme(ctx: &egui::Context) {
    let stroke = |w: f32, c: egui::Color32| egui::Stroke::new(w, c);
    let rounding = egui::Rounding::same(6.0);
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.faint_bg_color = SURFACE;
    v.extreme_bg_color = egui::Color32::from_rgb(0x07, 0x09, 0x08);
    v.hyperlink_color = BRAND;
    v.window_rounding = egui::Rounding::same(8.0);
    v.menu_rounding = rounding;
    v.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x34, 0xd3, 0x99, 48);
    v.selection.stroke = stroke(1.0, BRAND);

    v.widgets.noninteractive.bg_stroke = stroke(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = stroke(1.0, FG);
    v.widgets.noninteractive.rounding = rounding;
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.inactive.weak_bg_fill = SURFACE;
    v.widgets.inactive.bg_stroke = stroke(1.0, LINE);
    v.widgets.inactive.fg_stroke = stroke(1.0, FG);
    v.widgets.inactive.rounding = rounding;
    v.widgets.hovered.bg_fill = SURFACE_HOVER;
    v.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    v.widgets.hovered.bg_stroke = stroke(1.0, BRAND_DEEP);
    v.widgets.hovered.fg_stroke = stroke(1.5, BRAND);
    v.widgets.hovered.rounding = rounding;
    v.widgets.active.bg_fill = BRAND_DEEP;
    v.widgets.active.weak_bg_fill = BRAND_DEEP;
    v.widgets.active.bg_stroke = stroke(1.0, BRAND);
    v.widgets.active.fg_stroke = stroke(1.5, BG);
    v.widgets.active.rounding = rounding;
    v.widgets.open.bg_fill = SURFACE;
    v.widgets.open.weak_bg_fill = SURFACE;
    v.widgets.open.bg_stroke = stroke(1.0, BRAND_DEEP);
    v.widgets.open.fg_stroke = stroke(1.0, FG);
    v.widgets.open.rounding = rounding;

    ctx.set_visuals(v);
}

/// Screen title rendered in the brand's spectral green.
fn title(ui: &mut egui::Ui, text: &str) {
    ui.heading(egui::RichText::new(text).color(BRAND));
}

#[derive(PartialEq, Clone, Copy)]
enum Screen {
    NeedsToken,
    Characters,
    NewConnection,
    PickDiscovered,
    Password,
    Busy,
    Launched,
    Settings,
}

pub struct App {
    cmd_tx: Sender<Command>,
    evt_rx: Receiver<Event>,
    config: Config,
    token: Option<String>,

    screen: Screen,
    /// Where a Busy screen returns on a generic (non-token) failure.
    busy_return: Screen,
    /// Character we're trying to launch (drives the password screen + relaunch).
    launch_target: Option<Character>,

    // form fields
    token_input: String,
    account_input: String,
    password_input: String,
    game_input: String,
    save_discovered: bool,

    // data
    characters: Vec<Character>,
    discovered: Vec<SgeChar>,
    discovered_selected: Option<usize>,

    selected_char: Option<usize>,
    selected_fe: usize,

    // status
    stage: String,
    error: Option<String>,
    notice: Option<String>,
    launched: Option<(String, PathBuf, String)>, // (session_id, sal_path, frontend)
}

impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        apply_theme(ctx);
        let (cmd_tx, evt_rx) = worker::spawn(ctx.clone());
        let config = config::load_or_init();
        let token = keychain::get_token();

        let mut app = App {
            cmd_tx,
            evt_rx,
            config,
            token: token.clone(),
            screen: if token.is_some() {
                Screen::Busy
            } else {
                Screen::NeedsToken
            },
            busy_return: Screen::Characters,
            launch_target: None,
            token_input: String::new(),
            account_input: String::new(),
            password_input: String::new(),
            game_input: "DR".into(),
            save_discovered: true,
            characters: Vec::new(),
            discovered: Vec::new(),
            discovered_selected: None,
            selected_char: None,
            selected_fe: 0,
            stage: "Loading…".into(),
            error: None,
            notice: None,
            launched: None,
        };
        if token.is_some() {
            app.load_characters();
        }
        app
    }

    fn base(&self) -> Option<String> {
        self.config.api_base.clone()
    }
    fn token(&self) -> String {
        self.token.clone().unwrap_or_default()
    }

    fn load_characters(&mut self) {
        self.stage = "Loading characters…".into();
        self.busy_return = Screen::Characters;
        self.screen = Screen::Busy;
        let _ = self.cmd_tx.send(Command::LoadCharacters {
            base: self.base(),
            token: self.token(),
        });
    }

    /// Kick off a launch on the worker. `password = None` asks the worker to use the
    /// stored password (and emit NeedPassword if there isn't one).
    fn begin_launch(&mut self, character: Character, password: Option<String>) {
        if self.config.frontends.is_empty() {
            self.error = Some("No front end configured. Add one in Settings.".into());
            return;
        }
        if self.selected_fe >= self.config.frontends.len() {
            self.selected_fe = 0;
        }
        let frontend = self.config.frontends[self.selected_fe].clone();
        self.error = None;
        self.notice = None;
        self.stage = "Starting…".into();
        self.busy_return = Screen::Characters;
        self.screen = Screen::Busy;
        let _ = self.cmd_tx.send(Command::Launch {
            base: self.base(),
            token: self.token(),
            character,
            password,
            frontend,
            delete_session_on_exit: self.config.delete_session_on_exit,
            remember: self.config.remember_password,
        });
    }

    // ---- event handling ---------------------------------------------------

    fn drain_events(&mut self) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            match evt {
                Event::Stage(s) => self.stage = s,
                Event::Characters(chars) => {
                    self.characters = chars;
                    self.selected_char = None;
                    self.error = None;
                    self.screen = Screen::Characters;
                }
                Event::Discovered { account, game, chars } => {
                    self.account_input = account;
                    self.game_input = game;
                    self.discovered = chars;
                    self.discovered_selected = None;
                    self.error = None;
                    self.screen = Screen::PickDiscovered;
                }
                Event::Launched {
                    session_id,
                    sal_path,
                    frontend,
                } => {
                    self.password_input.clear();
                    self.launched = Some((session_id, sal_path, frontend));
                    self.error = None;
                    self.screen = Screen::Launched;
                }
                Event::NeedPassword => {
                    self.password_input.clear();
                    self.error = None;
                    self.screen = Screen::Password;
                }
                Event::SgeAuthFailed { account, message } => {
                    self.password_input.clear();
                    self.error =
                        Some(format!("Password for {account} failed: {message} — re-enter it."));
                    self.screen = Screen::Password;
                }
                Event::Failed {
                    message,
                    token_invalid,
                } => {
                    self.error = Some(message);
                    self.screen = if token_invalid {
                        Screen::NeedsToken
                    } else {
                        self.busy_return
                    };
                }
            }
        }
    }

    // ---- screens ----------------------------------------------------------

    fn ui_needs_token(&mut self, ui: &mut egui::Ui) {
        title(ui, "Connect to MUD Mobile");
        ui.add_space(6.0);
        ui.label("Paste your MUD Mobile device token (starts with \"wlk_\"). Create one at");
        ui.hyperlink("https://mudmobile.com");
        ui.label("under Tokens → New token.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Token:");
            ui.add(
                egui::TextEdit::singleline(&mut self.token_input)
                    .hint_text("wlk_…")
                    .desired_width(360.0),
            );
        });
        ui.add_space(8.0);
        let token = self.token_input.trim().to_string();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!token.is_empty(), egui::Button::new("Save & Connect"))
                .clicked()
            {
                match keychain::set_token(&token) {
                    Err(e) => self.error = Some(e.to_string()),
                    Ok(()) => {
                        self.token = Some(token.clone());
                        self.token_input.clear();
                        self.load_characters();
                    }
                }
            }
            if ui.button("⚙ Settings").clicked() {
                self.screen = Screen::Settings;
            }
        });
    }

    fn ui_characters(&mut self, ui: &mut egui::Ui) {
        title(ui, "Choose a character");
        ui.add_space(4.0);
        if self.characters.is_empty() {
            ui.label("No saved characters yet.");
        } else {
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for (i, c) in self.characters.iter().enumerate() {
                        let label = format!("{}   ({}, {})", c.character_name, c.game, c.account);
                        if ui
                            .selectable_label(self.selected_char == Some(i), label)
                            .clicked()
                        {
                            self.selected_char = Some(i);
                        }
                    }
                });
        }
        ui.add_space(8.0);
        self.ui_frontend_picker(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let can_play = self.selected_char.is_some() && !self.config.frontends.is_empty();
            if ui
                .add_enabled(can_play, egui::Button::new("Play ▶"))
                .clicked()
            {
                if let Some(i) = self.selected_char {
                    let character = self.characters[i].clone();
                    self.launch_target = Some(character.clone());
                    // None => use the stored password (worker prompts if there isn't one).
                    self.begin_launch(character, None);
                }
            }
            if ui.button("New connection…").clicked() {
                self.password_input.clear();
                self.discovered.clear();
                self.screen = Screen::NewConnection;
            }
            if ui.button("↻ Reload").clicked() {
                self.load_characters();
            }
            if ui.button("⚙ Settings").clicked() {
                self.screen = Screen::Settings;
            }
        });
    }

    /// Front-end combo box + a warning if the chosen FE speaks the WIZ protocol.
    fn ui_frontend_picker(&mut self, ui: &mut egui::Ui) {
        if self.selected_fe >= self.config.frontends.len() {
            self.selected_fe = 0;
        }
        let current = self
            .config
            .frontends
            .get(self.selected_fe)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "(none)".into());
        egui::ComboBox::from_label("Front end")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (i, f) in self.config.frontends.iter().enumerate() {
                    ui.selectable_value(&mut self.selected_fe, i, f.name.clone());
                }
            });
        if let Some(fe) = self.config.frontends.get(self.selected_fe) {
            if fe.protocol == Protocol::Wiz {
                ui.colored_label(
                    egui::Color32::from_rgb(210, 150, 0),
                    "⚠ Wizard uses the WIZ protocol; the hosted Stormfront stream may not be compatible.",
                );
            }
        }
    }

    fn ui_new_connection(&mut self, ui: &mut egui::Ui) {
        title(ui, "New connection");
        ui.label("Log in with your play.net account to discover its characters via SGE.");
        ui.label("Your password stays on this machine — only the character list is fetched.");
        ui.add_space(8.0);
        egui::Grid::new("newconn").num_columns(2).show(ui, |ui| {
            ui.label("Account:");
            ui.add(egui::TextEdit::singleline(&mut self.account_input).desired_width(260.0));
            ui.end_row();
            ui.label("Password:");
            ui.add(
                egui::TextEdit::singleline(&mut self.password_input)
                    .password(true)
                    .desired_width(260.0),
            );
            ui.end_row();
            ui.label("Game code:");
            ui.add(egui::TextEdit::singleline(&mut self.game_input).desired_width(120.0));
            ui.end_row();
        });
        ui.label("Game codes: DR / DRX / DRF / DRT (DragonRealms), GS3 / GSX / GSF (GemStone).");
        ui.checkbox(&mut self.save_discovered, "Save discovered characters to MUD Mobile");
        if ui
            .checkbox(&mut self.config.remember_password, "Remember password")
            .changed()
        {
            let _ = config::save(&self.config);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = !self.account_input.trim().is_empty()
                && !self.password_input.is_empty()
                && !self.game_input.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Discover")).clicked() {
                self.stage = "Discovering…".into();
                self.busy_return = Screen::NewConnection;
                self.screen = Screen::Busy;
                let _ = self.cmd_tx.send(Command::Discover {
                    base: self.base(),
                    token: self.token(),
                    account: self.account_input.trim().to_string(),
                    password: self.password_input.clone(),
                    game: self.game_input.trim().to_string(),
                    save: self.save_discovered,
                    remember: self.config.remember_password,
                });
            }
            if ui.button("Cancel").clicked() {
                self.password_input.clear();
                self.screen = Screen::Characters;
            }
        });
    }

    fn ui_pick_discovered(&mut self, ui: &mut egui::Ui) {
        title(ui, "Discovered characters");
        ui.label(format!(
            "Account {} ({}). Pick a character to play.",
            self.account_input, self.game_input
        ));
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                for (i, c) in self.discovered.iter().enumerate() {
                    if ui
                        .selectable_label(self.discovered_selected == Some(i), &c.name)
                        .clicked()
                    {
                        self.discovered_selected = Some(i);
                    }
                }
            });
        ui.add_space(8.0);
        self.ui_frontend_picker(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let can_play = self.discovered_selected.is_some() && !self.config.frontends.is_empty();
            if ui.add_enabled(can_play, egui::Button::new("Play ▶")).clicked() {
                let idx = self.discovered_selected.unwrap();
                let sc = &self.discovered[idx];
                let character = Character {
                    id: String::new(),
                    account: self.account_input.trim().to_string(),
                    game: self.game_input.trim().to_string(),
                    character_code: sc.code.clone(),
                    character_name: sc.name.clone(),
                    last_used_at: None,
                };
                self.launch_target = Some(character.clone());
                // We already hold the (just-validated) password from discovery.
                let pw = self.password_input.clone();
                self.begin_launch(character, Some(pw));
            }
            if ui.button("Back").clicked() {
                self.screen = Screen::Characters;
            }
        });
    }

    fn ui_password(&mut self, ui: &mut egui::Ui) {
        let name = self
            .launch_target
            .as_ref()
            .map(|c| format!("{} ({}, {})", c.character_name, c.game, c.account))
            .unwrap_or_default();
        title(ui, "Enter password");
        ui.label(format!("Logging in {name}."));
        ui.label("Saved to your OS keychain and reused next time (never sent to MUD Mobile).");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label("Password:");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.password_input)
                    .password(true)
                    .desired_width(220.0),
            );
            resp.request_focus();
            if ui
                .checkbox(&mut self.config.remember_password, "Remember")
                .changed()
            {
                let _ = config::save(&self.config);
            }
        });
        ui.add_space(8.0);
        self.ui_frontend_picker(ui);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = !self.password_input.is_empty() && self.launch_target.is_some();
            if ui.add_enabled(ready, egui::Button::new("Play ▶")).clicked() {
                if let Some(character) = self.launch_target.clone() {
                    let pw = self.password_input.clone();
                    self.begin_launch(character, Some(pw));
                }
            }
            if ui.button("Cancel").clicked() {
                self.password_input.clear();
                self.screen = Screen::Characters;
            }
        });
    }

    fn ui_busy(&mut self, ui: &mut egui::Ui) {
        ui.add_space(48.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(40.0).color(BRAND));
            ui.add_space(14.0);
            ui.label(egui::RichText::new(&self.stage).size(15.0).color(FG));
            ui.add_space(4.0);
            ui.colored_label(MUTED, "This can take up to a minute while the cloud runner boots.");
        });
    }

    fn ui_launched(&mut self, ui: &mut egui::Ui) {
        title(ui, "Launched ✓");
        if let Some((session_id, sal_path, fe)) = &self.launched {
            ui.label(format!("Started {fe}. The game window should be opening."));
            ui.add_space(4.0);
            ui.label(format!("Session: {session_id}"));
            ui.label(format!("Launch file: {}", sal_path.display()));
        }
        ui.add_space(12.0);
        if ui.button("Back to characters").clicked() {
            self.screen = Screen::Characters;
        }
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        title(ui, "Settings");
        egui::ScrollArea::vertical().show(ui, |ui| {
            // --- token ---
            ui.collapsing("MUD Mobile token", |ui| {
                ui.label(if self.token.is_some() {
                    "A token is stored in your OS keychain."
                } else {
                    "No token stored."
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.token_input)
                            .hint_text("wlk_…")
                            .desired_width(300.0),
                    );
                    let t = self.token_input.trim().to_string();
                    if ui.add_enabled(!t.is_empty(), egui::Button::new("Save token")).clicked() {
                        match keychain::set_token(&t) {
                            Ok(()) => {
                                self.token = Some(t);
                                self.token_input.clear();
                            }
                            Err(e) => self.error = Some(e.to_string()),
                        }
                    }
                });
                if ui.button("Clear token").clicked() {
                    let _ = keychain::delete_token();
                    self.token = None;
                }
            });

            // --- preferences ---
            ui.collapsing("Preferences", |ui| {
                ui.checkbox(
                    &mut self.config.delete_session_on_exit,
                    "End the hosted session when the front end closes",
                );
                ui.horizontal(|ui| {
                    ui.label("API base (advanced):");
                    let mut base = self.config.api_base.clone().unwrap_or_default();
                    if ui
                        .add(egui::TextEdit::singleline(&mut base).hint_text("https://mudmobile.com"))
                        .changed()
                    {
                        self.config.api_base = if base.trim().is_empty() {
                            None
                        } else {
                            Some(base.trim().to_string())
                        };
                    }
                });
            });

            // --- saved passwords ---
            ui.collapsing("Saved passwords", |ui| {
                let mut accounts: Vec<String> =
                    self.characters.iter().map(|c| c.account.clone()).collect();
                accounts.sort();
                accounts.dedup();
                if accounts.is_empty() {
                    ui.label("No accounts known yet — passwords are remembered after you log in.");
                } else {
                    ui.label("Forget a saved password (you'll be asked for it on next launch):");
                    let mut forget: Option<String> = None;
                    for acct in &accounts {
                        ui.horizontal(|ui| {
                            ui.label(acct);
                            if ui.button("Forget").clicked() {
                                forget = Some(acct.clone());
                            }
                        });
                    }
                    if let Some(acct) = forget {
                        let _ = keychain::delete_password(&acct);
                        self.notice = Some(format!("Forgot the saved password for {acct}."));
                    }
                }
            });

            // --- front ends ---
            ui.collapsing("Front ends", |ui| {
                let mut remove: Option<usize> = None;
                for i in 0..self.config.frontends.len() {
                    ui.separator();
                    let fe = &mut self.config.frontends[i];
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(&mut fe.name).desired_width(180.0));
                        egui::ComboBox::from_id_salt(("proto", i))
                            .selected_text(match fe.protocol {
                                Protocol::Storm => "Storm",
                                Protocol::Wiz => "Wiz",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut fe.protocol, Protocol::Storm, "Storm");
                                ui.selectable_value(&mut fe.protocol, Protocol::Wiz, "Wiz");
                            });
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Command (%1 = .sal):");
                        ui.add(
                            egui::TextEdit::singleline(&mut fe.command_template)
                                .desired_width(220.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Executable:");
                        let mut p = fe.paths.for_current_os().unwrap_or("").to_string();
                        if ui
                            .add(egui::TextEdit::singleline(&mut p).desired_width(300.0))
                            .changed()
                        {
                            fe.paths
                                .set_for_current_os(if p.is_empty() { None } else { Some(p.clone()) });
                        }
                        if ui.button("Browse…").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                fe.paths
                                    .set_for_current_os(Some(path.to_string_lossy().to_string()));
                            }
                        }
                    });
                }
                if let Some(i) = remove {
                    self.config.frontends.remove(i);
                }
                ui.separator();
                if ui.button("➕ Add front end").clicked() {
                    self.config.frontends.push(FrontEnd {
                        name: "New front end".into(),
                        protocol: Protocol::Storm,
                        command_template: "%1".into(),
                        working_dir: None,
                        paths: PerOsPath::default(),
                        sal_overrides: SalOverrides::default(),
                    });
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("💾 Save settings").clicked() {
                    if let Err(e) = config::save(&self.config) {
                        self.error = Some(e.to_string());
                    }
                }
                if ui.button("Done").clicked() {
                    let _ = config::save(&self.config);
                    self.screen = if self.token.is_some() {
                        Screen::Characters
                    } else {
                        Screen::NeedsToken
                    };
                }
            });
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();

        // Keep the spinner animating while work is in flight.
        if self.screen == Screen::Busy {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Settings / Reload live on each screen's action row (no redundant top bar).

        // Dismiss pinned right; the message wraps into the remaining width (right_to_left
        // places the button first, then the wrapped label fills the space to its left) so
        // long errors don't run off the window edge — the panel grows to fit.
        if let Some(err) = self.error.clone() {
            egui::TopBottomPanel::bottom("err").show(ctx, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Dismiss").clicked() {
                        self.error = None;
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("⚠ {err}"))
                                .color(egui::Color32::from_rgb(220, 80, 80)),
                        )
                        .wrap(),
                    );
                });
            });
        } else if let Some(note) = self.notice.clone() {
            egui::TopBottomPanel::bottom("note").show(ctx, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Dismiss").clicked() {
                        self.notice = None;
                    }
                    ui.add(
                        egui::Label::new(egui::RichText::new(format!("✓ {note}")).color(BRAND))
                            .wrap(),
                    );
                });
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.screen {
            Screen::NeedsToken => self.ui_needs_token(ui),
            Screen::Characters => self.ui_characters(ui),
            Screen::NewConnection => self.ui_new_connection(ui),
            Screen::PickDiscovered => self.ui_pick_discovered(ui),
            Screen::Password => self.ui_password(ui),
            Screen::Busy => self.ui_busy(ui),
            Screen::Launched => self.ui_launched(ui),
            Screen::Settings => self.ui_settings(ui),
        });
    }
}
