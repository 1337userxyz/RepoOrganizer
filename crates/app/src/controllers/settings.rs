use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use kit_dialog::PickRequest;
use kit_slint::PickRequestSlintExt;
use repoorganizer_core::paths::PathMapping;
use repoorganizer_core::stats::days_from_civil;
use slint::{ComponentHandle, SharedString};

use crate::state::{save_local, save_shared, Shared, State};
use crate::ui_sync::{apply, apply_theme_all, sync_settings_fields};
use crate::{AppTheme, MainWindow, SettingsWindow};

fn run_git_credential(git_path: Option<&str>, subcmd: &str, stdin_payload: &str) -> Result<String, String> {
    let bin = git_path.unwrap_or("git");
    let mut child = Command::new(bin)
        .args(["credential", subcmd])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git credential {subcmd} fehlgeschlagen: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_payload.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git credential {subcmd} Fehler: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn read_github_credential(git_path: Option<&str>) -> Option<(String, String)> {
    let out = run_git_credential(git_path, "fill", "protocol=https\nhost=github.com\n\n").ok()?;
    let mut user = String::new();
    let mut pass = String::new();
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("username=") {
            user = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("password=") {
            pass = v.trim().to_string();
        }
    }
    if pass.is_empty() { None } else { Some((user, pass)) }
}

fn mask_token(tok: &str) -> String {
    let chars: Vec<char> = tok.chars().collect();
    if chars.len() <= 8 {
        "••••••••".into()
    } else {
        let head: String = chars.iter().take(4).collect();
        let tail: String = chars.iter().skip(chars.len() - 4).collect();
        format!("{head}••••••••{tail}")
    }
}

fn token_expiry_status(expires_at: Option<&str>) -> (String, i32) {
    let Some(raw) = expires_at.map(str::trim).filter(|s| !s.is_empty()) else {
        return ("Kein Ablaufdatum hinterlegt".into(), 0);
    };
    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 3 {
        return ("Ungültiges Datum (Format: YYYY-MM-DD)".into(), 2);
    }
    let (Ok(y), Ok(m), Ok(d)) = (parts[0].parse::<i64>(), parts[1].parse::<u32>(), parts[2].parse::<u32>()) else {
        return ("Ungültiges Datum (Format: YYYY-MM-DD)".into(), 2);
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return ("Ungültiges Datum (Format: YYYY-MM-DD)".into(), 2);
    }
    let exp_day = days_from_civil(y, m, d);
    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let today_day = now_secs.div_euclid(86_400);
    let left = exp_day - today_day;
    if left < 0 {
        (format!("Abgelaufen vor {} Tag(en) ({raw})", -left), 3)
    } else if left == 0 {
        (format!("Läuft heute ab ({raw})!"), 3)
    } else if left <= 14 {
        (format!("Läuft in {left} Tag(en) ab ({raw})"), 2)
    } else {
        (format!("Gültig bis {raw} (noch {left} Tage)"), 1)
    }
}

pub fn sync_token_fields(cfgw: &SettingsWindow, st: &State) {
    let cred = read_github_credential(st.local.git_path.as_deref());
    match cred {
        Some((u, p)) => {
            cfgw.set_cfg_token_masked(mask_token(&p).into());
            let eff_user = if !u.is_empty() { u } else { st.local.git_user.clone() };
            cfgw.set_cfg_token_user(eff_user.into());
        }
        None => {
            cfgw.set_cfg_token_masked("Kein Token gespeichert".into());
            cfgw.set_cfg_token_user(st.local.git_user.as_str().into());
        }
    }
    let exp = st.local.token_expires_at.as_deref().unwrap_or("");
    cfgw.set_cfg_token_expires(exp.into());
    let (info, level) = token_expiry_status(st.local.token_expires_at.as_deref());
    cfgw.set_cfg_token_expiry_info(info.into());
    cfgw.set_cfg_token_expiry_level(level);
}

pub fn register(ui: &MainWindow, state: &Shared, settings_win: &SettingsWindow) {
    // Open Settings dialog from MainWindow
    {
        let (cfgw, state) = (settings_win.as_weak(), state.clone());
        ui.on_open_settings(move || {
            if let Some(w) = cfgw.upgrade() {
                let st = state.lock().unwrap();
                sync_settings_fields(&w, &st);
                w.global::<AppTheme>().invoke_apply(st.local.theme.as_str().into());
                let sz = w.window().size();
                if sz.width < 920 || sz.height < 640 {
                    w.window().set_size(slint::LogicalSize::new(980.0, 700.0));
                }
                let _ = w.show();
            }
        });
    }

    // SettingsWindow Callbacks
    {
        let cfgw = settings_win.as_weak();
        settings_win.on_close_window(move || {
            if let Some(w) = cfgw.upgrade() {
                let _ = w.hide();
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_theme_changed(move |theme: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.local.theme = theme.trim().to_string();
                save_local(&mut st);
                apply_theme_all(&ui, &st);
            }
        });
    }
    {
        let (weak, cfgw, state) = (ui.as_weak(), settings_win.as_weak(), state.clone());
        settings_win.on_save_settings(move |theme: SharedString, git_user: SharedString, git: SharedString, refr: SharedString, timeout: SharedString, stale: SharedString, shared_win: SharedString, shared_unix: SharedString, local_linux: SharedString, local_win: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.local.theme = theme.trim().to_string();
                st.local.git_user = git_user.trim().to_string();
                let g = git.trim();
                st.local.git_path = if g.is_empty() { None } else { Some(g.to_string()) };
                if let Ok(v) = refr.trim().parse() {
                    st.local.refresh_interval_secs = v;
                }
                if let Ok(v) = timeout.trim().parse() {
                    st.local.reach_timeout_secs = v;
                }
                if let Ok(v) = stale.trim().parse() {
                    st.local.stale_days = v;
                }
                let sw = shared_win.trim();
                st.shared.base_paths.shared_windows = if sw.is_empty() { None } else { Some(sw.to_string()) };
                let su = shared_unix.trim();
                st.shared.base_paths.shared_unix = if su.is_empty() { None } else { Some(su.to_string()) };
                let ll = local_linux.trim();
                st.shared.base_paths.local_linux = if ll.is_empty() { None } else { Some(ll.to_string()) };
                let lw = local_win.trim();
                st.shared.base_paths.local_windows = if lw.is_empty() { None } else { Some(lw.to_string()) };
                if let Some(w) = cfgw.upgrade() {
                    st.shared.base_paths.sync_shared_windows = w.get_cfg_sync_shared_win();
                    st.shared.base_paths.sync_shared_unix = w.get_cfg_sync_shared_unix();
                    st.shared.base_paths.sync_local_linux = w.get_cfg_sync_local_linux();
                    st.shared.base_paths.sync_local_windows = w.get_cfg_sync_local_win();
                }

                save_local(&mut st);
                save_shared(&mut st);
                st.task = "Einstellungen gespeichert".into();
                if let Some(w) = cfgw.upgrade() {
                    sync_settings_fields(&w, &st);
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let cfgw = settings_win.as_weak();
        settings_win.on_browse_base_path(move |field: SharedString| {
            let target = field.to_string();
            PickRequest::folder()
                .title("Basispfad auswählen")
                .context("base_path")
                .pick_into(cfgw.clone(), move |w, folder| {
                    let path_str: SharedString = folder.to_string_lossy().to_string().into();
                    match target.as_str() {
                        "shared_unix" => w.set_cfg_shared_unix(path_str),
                        "shared_win" => w.set_cfg_shared_win(path_str),
                        "local_linux" => w.set_cfg_local_linux(path_str),
                        "local_win" => w.set_cfg_local_win(path_str),
                        _ => {}
                    }
                });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_add_mapping(move |win: SharedString, unix: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                st.shared.mappings.push(PathMapping { windows: win.trim().to_string(), unix: unix.trim().to_string() });
                save_shared(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_remove_mapping(move |idx: i32| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                let i = idx as usize;
                if i < st.shared.mappings.len() {
                    st.shared.mappings.remove(i);
                    save_shared(&mut st);
                }
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        settings_win.on_toggle_health_check(move |id: SharedString| {
            if let Some(ui) = weak.upgrade() {
                let mut st = state.lock().unwrap();
                let id_s = id.to_string();
                if let Some(pos) = st.local.health.disabled.iter().position(|x| x == &id_s) {
                    st.local.health.disabled.remove(pos);
                } else {
                    st.local.health.disabled.push(id_s);
                }
                save_local(&mut st);
                apply(&ui, &mut st);
            }
        });
    }
    {
        let (cfgw, state) = (settings_win.as_weak(), state.clone());
        settings_win.on_save_github_token(move |user: SharedString, token: SharedString, expires: SharedString| {
            let Some(w) = cfgw.upgrade() else { return };
            let mut st = state.lock().unwrap();
            let u = user.trim().to_string();
            let tok = token.trim().to_string();
            let exp = expires.trim().to_string();
            st.local.token_expires_at = if exp.is_empty() { None } else { Some(exp) };
            save_local(&mut st);

            if !tok.is_empty() {
                let bin = st.local.git_path.as_deref().unwrap_or("git");
                #[cfg(not(target_os = "windows"))]
                let _ = Command::new(bin)
                    .args(["config", "--global", "--replace-all", "credential.https://github.com.helper", "store"])
                    .output();

                let eff_user = if !u.is_empty() {
                    u.clone()
                } else if !st.local.git_user.is_empty() {
                    st.local.git_user.clone()
                } else {
                    "git".to_string()
                };
                // Alten Eintrag entfernen, damit keine Duplikate entstehen
                let _ = run_git_credential(
                    st.local.git_path.as_deref(),
                    "reject",
                    &format!("protocol=https\nhost=github.com\nusername={eff_user}\n\n"),
                );
                let payload = format!("protocol=https\nhost=github.com\nusername={eff_user}\npassword={tok}\n\n");
                match run_git_credential(st.local.git_path.as_deref(), "approve", &payload) {
                    Ok(_) => {
                        w.set_cfg_token_input("".into());
                        w.set_cfg_token_feedback("Token & Ablaufdatum gespeichert.".into());
                    }
                    Err(e) => {
                        w.set_cfg_token_feedback(format!("Fehler beim Speichern: {e}").into());
                    }
                }
            } else {
                w.set_cfg_token_feedback("Ablaufdatum gespeichert.".into());
            }
            sync_token_fields(&w, &st);
        });
    }
    {
        let (cfgw, state) = (settings_win.as_weak(), state.clone());
        settings_win.on_remove_github_token(move || {
            let Some(w) = cfgw.upgrade() else { return };
            let mut st = state.lock().unwrap();
            let existing = read_github_credential(st.local.git_path.as_deref());
            if let Some((u, _)) = existing {
                let payload = if u.is_empty() {
                    "protocol=https\nhost=github.com\n\n".to_string()
                } else {
                    format!("protocol=https\nhost=github.com\nusername={u}\n\n")
                };
                let _ = run_git_credential(st.local.git_path.as_deref(), "reject", &payload);
            }
            st.local.token_expires_at = None;
            save_local(&mut st);
            w.set_cfg_token_input("".into());
            w.set_cfg_token_feedback("Token aus lokalem Speicher entfernt.".into());
            sync_token_fields(&w, &st);
        });
    }
}
