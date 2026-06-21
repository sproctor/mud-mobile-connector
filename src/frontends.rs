//! Front-end registry: the default (user-editable) list, executable discovery,
//! command templating, and process spawn. Mirrors Lich's launch behaviour
//! (`lib/main/main.rb` ~256-286: `%1` placeholder for the .sal path, `/`->`\`
//! path swap on Windows) and per-FE `.sal` overrides (`launch_data.rb`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::error::{AppError, AppResult};
use crate::model::{FrontEnd, PerOsPath, Protocol, SalOverrides};

/// The default front ends shipped on first run. The user can add/edit/remove these
/// in Settings; the list is persisted in config.toml.
pub fn default_frontends() -> Vec<FrontEnd> {
    vec![
        FrontEnd {
            name: "Wrayth / StormFront".into(),
            protocol: Protocol::Storm,
            paths: PerOsPath {
                windows: Some(r"C:\Program Files (x86)\Simutronics\Wrayth\Wrayth.exe".into()),
                ..Default::default()
            },
            command_template: "%1".into(),
            sal_overrides: SalOverrides::default(),
            working_dir: None,
        },
        FrontEnd {
            name: "Warlock".into(),
            protocol: Protocol::Storm,
            paths: PerOsPath::default(),
            command_template: "%1".into(),
            sal_overrides: SalOverrides::default(),
            working_dir: None,
        },
        FrontEnd {
            name: "Genie".into(),
            protocol: Protocol::Storm,
            paths: PerOsPath {
                windows: Some(r"C:\Program Files (x86)\GenieClient\Genie.exe".into()),
                ..Default::default()
            },
            command_template: "%1".into(),
            sal_overrides: SalOverrides::default(),
            working_dir: None,
        },
        FrontEnd {
            name: "Mudlet".into(),
            protocol: Protocol::Storm,
            paths: PerOsPath::default(),
            command_template: "%1".into(),
            sal_overrides: SalOverrides::default(),
            working_dir: None,
        },
        FrontEnd {
            name: "Wizard".into(),
            protocol: Protocol::Wiz,
            paths: PerOsPath {
                windows: Some(r"C:\Program Files (x86)\Simutronics\Wizard\WIZARD.EXE".into()),
                ..Default::default()
            },
            command_template: "%1".into(),
            // Mirror Lich's wizard rewrites (launch_data.rb:23-27).
            sal_overrides: SalOverrides {
                game: Some("WIZ".into()),
                gamefile: Some("WIZARD.EXE".into()),
                fullgamename: Some("Wizard Front End".into()),
            },
            working_dir: None,
        },
    ]
}

/// Built-in executable name candidates to search on PATH when no explicit path is set.
fn discovery_candidates(name: &str) -> &'static [&'static str] {
    match name {
        "Wrayth / StormFront" => &["Wrayth.exe", "Wrayth", "StormFront.exe"],
        "Warlock" => &["warlock", "Warlock", "warlock.exe"],
        "Genie" => &["Genie.exe", "Genie"],
        "Mudlet" => &["mudlet", "Mudlet", "mudlet.exe"],
        "Wizard" => &["WIZARD.EXE", "Wizard.exe"],
        _ => &[],
    }
}

/// Resolve the executable for a front end: the explicitly-configured path if it
/// exists, otherwise a best-effort search of well-known names on PATH.
pub fn resolve_exe(fe: &FrontEnd) -> Option<PathBuf> {
    if let Some(p) = fe.paths.for_current_os() {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    for cand in discovery_candidates(&fe.name) {
        let pb = PathBuf::from(cand);
        if pb.is_absolute() && pb.exists() {
            return Some(pb);
        }
        if let Some(found) = find_on_path(cand) {
            return Some(found);
        }
    }
    None
}

/// Look up an executable name on the PATH environment variable.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Normalize the .sal path for inclusion in the command line. On Windows, Lich
/// swaps `/` for `\` (main.rb:274); elsewhere the path is used verbatim.
fn normalize_sal_path(sal_path: &Path) -> String {
    let s = sal_path.to_string_lossy().to_string();
    if cfg!(target_os = "windows") {
        s.replace('/', "\\")
    } else {
        s
    }
}

/// Render command-template arguments, substituting `%1` with the (normalized) .sal path.
/// Whitespace-separated tokens; `%1` anywhere in a token is replaced.
fn render_args(template: &str, sal_path_str: &str) -> Vec<String> {
    template
        .split_whitespace()
        .map(|tok| tok.replace("%1", sal_path_str))
        .collect()
}

/// Build the (executable, args) pair to launch a front end with the given .sal.
pub fn build_command(fe: &FrontEnd, sal_path: &Path) -> AppResult<(PathBuf, Vec<String>)> {
    let exe = resolve_exe(fe).ok_or_else(|| {
        let hint = fe
            .paths
            .for_current_os()
            .map(|p| format!(" (configured: {p})"))
            .unwrap_or_default();
        AppError::FrontEndNotFound(format!("{}{}", fe.name, hint))
    })?;
    let args = render_args(&fe.command_template, &normalize_sal_path(sal_path));
    Ok((exe, args))
}

/// Spawn the front end as a detached child process, handing it the .sal path.
pub fn spawn(fe: &FrontEnd, sal_path: &Path) -> AppResult<Child> {
    let (exe, args) = build_command(fe, sal_path)?;
    let mut cmd = Command::new(&exe);
    cmd.args(&args);
    if let Some(dir) = &fe.working_dir {
        cmd.current_dir(dir);
    }
    cmd.spawn().map_err(|e| {
        AppError::FrontEndNotFound(format!("{} ({}): {}", fe.name, exe.display(), e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_present() {
        let names: Vec<_> = default_frontends().iter().map(|f| f.name.clone()).collect();
        assert!(names.contains(&"Warlock".to_string()));
        assert!(names.contains(&"Wizard".to_string()));
        assert_eq!(default_frontends().len(), 5);
    }

    #[test]
    fn wizard_has_wiz_overrides() {
        let wiz = default_frontends()
            .into_iter()
            .find(|f| f.name == "Wizard")
            .unwrap();
        assert_eq!(wiz.protocol, Protocol::Wiz);
        assert_eq!(wiz.sal_overrides.game.as_deref(), Some("WIZ"));
        assert_eq!(wiz.sal_overrides.gamefile.as_deref(), Some("WIZARD.EXE"));
    }

    #[test]
    fn render_args_substitutes_placeholder() {
        assert_eq!(render_args("%1", "/tmp/x.sal"), vec!["/tmp/x.sal"]);
        assert_eq!(
            render_args("--connect %1 --verbose", "/tmp/x.sal"),
            vec!["--connect", "/tmp/x.sal", "--verbose"]
        );
        assert_eq!(render_args("/sal:%1", "/tmp/x.sal"), vec!["/sal:/tmp/x.sal"]);
    }

    #[test]
    fn render_args_no_placeholder_passes_template() {
        assert_eq!(render_args("--foo", "/tmp/x.sal"), vec!["--foo"]);
    }
}
