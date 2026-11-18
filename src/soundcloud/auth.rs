use super::*;
use std::sync::OnceLock;

fn keyring_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("secret-tool")
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    })
}

pub(crate) fn validate_token(token: &str) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("Токен пустой.".into());
    }
    if token.len() > 4096 {
        return Err("Токен слишком длинный.".into());
    }
    if token.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Токен не должен содержать пробелы и управляющие символы.".into());
    }
    Ok(())
}

pub(super) fn load_stored_token() -> Option<String> {
    if !keyring_available() {
        return None;
    }
    let output = Command::new("secret-tool")
        .args(["lookup", "service", TOKEN_SERVICE, "kind", TOKEN_KIND])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

pub(crate) fn save_token(token: &str) -> Result<(), String> {
    validate_token(token)?;
    if !keyring_available() {
        return Err("Связка ключей недоступна (нет secret-tool / libsecret).".into());
    }
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label",
            "Own Player SoundCloud",
            "service",
            TOKEN_SERVICE,
            "kind",
            TOKEN_KIND,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Не удалось запустить secret-tool: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(token.trim().as_bytes())
            .map_err(|error| format!("Не удалось передать токен: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("secret-tool: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Не удалось сохранить токен в связке ключей: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

pub(crate) fn clear_stored_token() -> Result<(), String> {
    if !keyring_available() {
        return Ok(());
    }
    let output = Command::new("secret-tool")
        .args(["clear", "service", TOKEN_SERVICE, "kind", TOKEN_KIND])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("secret-tool: {error}"))?;
    if output.status.success() || (output.status.code() == Some(1) && output.stderr.is_empty()) {
        Ok(())
    } else {
        Err("Не удалось удалить токен из связки ключей.".into())
    }
}

pub(crate) fn set_session_token(token: Option<String>) {
    if let Ok(mut guard) = SESSION.lock() {
        *guard = token;
    }
}

fn session_token() -> Option<String> {
    SESSION.lock().ok().and_then(|guard| guard.clone())
}

pub(crate) fn get_token() -> Option<String> {
    session_token().or_else(load_stored_token)
}

pub(crate) fn token_summary() -> &'static str {
    if load_stored_token().is_some() {
        "Токен сохранён в связке ключей"
    } else if session_token().is_some() {
        "Токен задан на этот сеанс"
    } else {
        "Токен не задан"
    }
}
