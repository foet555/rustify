// Platform gate: the player only unlocks on machines that actually have the
// Spotify desktop app installed. Every probe records evidence so the gate
// screen can say exactly what was looked for and what was missing.
use std::path::PathBuf;

pub struct Evidence {
    pub label: String,
    pub ok: bool,
}

pub struct Presence {
    pub found: bool,
    pub evidence: Vec<Evidence>,
}

pub fn verdict(evidence: Vec<Evidence>) -> Presence {
    let found = evidence.iter().any(|e| e.ok);
    Presence { found, evidence }
}

fn probe_path(label: &str, path: PathBuf) -> Evidence {
    Evidence {
        label: label.to_string(),
        ok: path.is_file(),
    }
}

#[cfg(windows)]
fn run(program: &str, args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output().ok().map(|o| {
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
        )
    })
}

#[cfg(windows)]
fn registry_probe(label: &str, key: &str) -> Evidence {
    let ok = matches!(run("reg", &["query", key]), Some((true, _)));
    Evidence {
        label: label.to_string(),
        ok,
    }
}

#[cfg(windows)]
fn where_probe(label: &str) -> Evidence {
    let ok = matches!(run("where", &["spotify.exe"]), Some((true, _)));
    Evidence {
        label: label.to_string(),
        ok,
    }
}

#[cfg(windows)]
fn running_probe(label: &str) -> Evidence {
    let ok = match run("tasklist", &["/FI", "IMAGENAME eq Spotify.exe", "/NH"]) {
        Some((true, out)) => out.to_lowercase().contains("spotify.exe"),
        _ => false,
    };
    Evidence {
        label: label.to_string(),
        ok,
    }
}

#[cfg(windows)]
fn store_probe(root: &PathBuf) -> Evidence {
    let windows_apps = root.join("Microsoft").join("WindowsApps");
    let mut ok = false;
    if let Ok(entries) = std::fs::read_dir(&windows_apps) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.starts_with("spotifyab.spotifymusic") && entry.path().join("Spotify.exe").is_file()
            {
                ok = true;
                break;
            }
        }
    }
    Evidence {
        label: format!("Microsoft Store package in {}", windows_apps.display()),
        ok,
    }
}

pub fn check() -> Presence {
    check_with(forced_missing())
}

/// Verification-only hook: `RUSTIFY_GATE_FORCE=missing` makes every probe report
/// missing so the locked path can be exercised end to end. It can only lock the
/// app, never unlock it, so it is not a bypass.
pub fn forced_missing() -> bool {
    std::env::var("RUSTIFY_GATE_FORCE")
        .map(|v| v.eq_ignore_ascii_case("missing"))
        .unwrap_or(false)
}

pub fn check_with(force_missing: bool) -> Presence {
    let mut presence = probe_real();
    if force_missing {
        for e in &mut presence.evidence {
            e.ok = false;
        }
        presence.found = false;
        presence.evidence.push(Evidence {
            label: "Forced missing by RUSTIFY_GATE_FORCE (verification only)".to_string(),
            ok: false,
        });
    }
    presence
}

fn probe_real() -> Presence {
    #[cfg(windows)]
    {
        let mut evidence = Vec::new();

        if let Ok(appdata) = std::env::var("APPDATA") {
            let base = PathBuf::from(appdata);
            evidence.push(probe_path(
                &format!(
                    "Per-user install ({})",
                    base.join("Spotify").join("Spotify.exe").display()
                ),
                base.join("Spotify").join("Spotify.exe"),
            ));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let base = PathBuf::from(local);
            evidence.push(probe_path(
                &format!(
                    "Local install ({})",
                    base.join("Spotify").join("Spotify.exe").display()
                ),
                base.join("Spotify").join("Spotify.exe"),
            ));
            evidence.push(probe_path(
                "Store app execution alias",
                base.join("Microsoft").join("WindowsApps").join("Spotify.exe"),
            ));
            evidence.push(store_probe(&base));
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(dir) = std::env::var(var) {
                let base = PathBuf::from(dir).join("Spotify");
                evidence.push(probe_path(
                    &format!("Program Files install ({})", base.display()),
                    base.join("Spotify.exe"),
                ));
            }
        }
        evidence.push(registry_probe(
            "Uninstall entry (current user)",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Spotify",
        ));
        evidence.push(registry_probe(
            "Uninstall entry (all users)",
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Spotify",
        ));
        evidence.push(registry_probe(
            "App Paths registration",
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\App Paths\Spotify.exe",
        ));
        evidence.push(where_probe("Spotify on PATH"));
        evidence.push(running_probe("Spotify currently running"));

        return verdict(evidence);
    }
    #[cfg(not(windows))]
    {
        verdict(vec![Evidence {
            label: "Unsupported platform".to_string(),
            ok: false,
        }])
    }
}

/// Launch verdict for the local player: Rustify plays the owner's own files
/// with no Spotify install or sign-in, so the window never waits on Spotify.
/// The detailed probes stay available through `check()`.
pub fn check_local_player() -> Presence {
    verdict(vec![Evidence {
        label: "Local player (Spotify is optional)".to_string(),
        ok: true,
    }])
}

pub fn report(p: &Presence) -> String {
    let mut out = String::new();
    out.push_str(&format!("spotify_installed={}\n", p.found));
    for e in &p.evidence {
        out.push_str(&format!(
            "{}: {}\n",
            if e.ok { "ok     " } else { "missing" },
            e.label
        ));
    }
    out
}

pub fn open_url(url: &str) {
    crate::spotify::open_browser(url);
}

pub fn open_download_page() {
    open_url("https://www.spotify.com/download/windows/");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_needs_at_least_one_pass() {
        let none = verdict(vec![
            Evidence {
                label: "a".to_string(),
                ok: false,
            },
            Evidence {
                label: "b".to_string(),
                ok: false,
            },
        ]);
        assert!(!none.found);
        assert_eq!(none.evidence.len(), 2);

        let one = verdict(vec![
            Evidence {
                label: "a".to_string(),
                ok: false,
            },
            Evidence {
                label: "b".to_string(),
                ok: true,
            },
        ]);
        assert!(one.found);
    }

    #[test]
    fn path_probe_only_accepts_real_files() {
        let path = std::env::temp_dir().join(format!(
            "rustify-gate-{}-{}.exe",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        assert!(!probe_path("missing", path.clone()).ok);
        std::fs::write(&path, b"stub").unwrap();
        let hit = probe_path("present", path.clone());
        assert!(hit.ok);
        assert!(hit.label.contains("present"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn forced_missing_locks_the_gate_without_a_bypass() {
        let locked = check_with(true);
        assert!(!locked.found, "forcing must lock the gate");
        assert!(locked.evidence.iter().all(|e| !e.ok));
        assert!(locked
            .evidence
            .iter()
            .any(|e| e.label.contains("RUSTIFY_GATE_FORCE")));
        // Default path stays honest: forcing cannot unlock a machine that fails.
        let real = check_with(false);
        assert_eq!(real.found, real.evidence.iter().any(|e| e.ok));
    }

    #[test]
    fn check_collects_evidence_without_panicking() {
        let p = check();
        assert!(!p.evidence.is_empty());
        assert_eq!(p.found, p.evidence.iter().any(|e| e.ok));
        assert!(report(&p).contains("spotify_installed="));
    }

    #[test]
    fn the_local_player_opens_without_a_spotify_install() {
        let p = check_local_player();
        assert!(p.found);
        assert_eq!(p.evidence.len(), 1);
        assert!(p.evidence[0].ok);
    }
}
