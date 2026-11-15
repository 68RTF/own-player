use super::*;
use std::sync::OnceLock;
#[derive(Clone, Debug)]
pub(super) struct ParsedUrl {
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) port: Option<u16>,
    pub(super) path: String,
}

impl ParsedUrl {
    pub(super) fn origin(&self) -> String {
        match self.port {
            Some(port) => format!("{}://{}:{port}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }
}

pub(super) fn parse_url(input: &str) -> Option<ParsedUrl> {
    let (scheme, rest) = input.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() || authority.contains(['@', '\\', '%']) {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port.parse::<u16>().ok()?)),
        None => (authority, None),
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return None;
    }
    let path = match &rest[end..] {
        "" => "/",
        value if value.starts_with('#') => "/",
        value => value,
    };
    Some(ParsedUrl {
        scheme,
        host: host.to_ascii_lowercase(),
        port,
        path: path.split('#').next().unwrap_or(path).to_string(),
    })
}

pub(super) fn is_soundcloud_host(host: &str) -> bool {
    host == "soundcloud.com" || host.ends_with(".soundcloud.com")
}

pub(super) fn is_sndcdn_host(host: &str) -> bool {
    host == "sndcdn.com" || host.ends_with(".sndcdn.com")
}

pub(super) fn is_short_host(host: &str) -> bool {
    matches!(host, "on.soundcloud.com" | "soundcloud.app.goo.gl")
}

pub(super) fn is_allowed_download_host(host: &str) -> bool {
    is_soundcloud_host(host) || is_sndcdn_host(host)
}

pub(super) fn normalize_track_url(input: &str, allow_http: bool) -> Result<String, ScError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ScError::InvalidUrl("ссылка пустая"));
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut parsed =
        parse_url(&with_scheme).ok_or(ScError::InvalidUrl("не удалось разобрать адрес"))?;
    if parsed.scheme == "http" && !allow_http {
        parsed.scheme = "https".into();
    }
    if parsed.scheme != "https" && !(allow_http && parsed.scheme == "http") {
        return Err(ScError::InvalidUrl("нужен адрес https://"));
    }
    if !is_soundcloud_host(&parsed.host) && !is_short_host(&parsed.host) {
        return Err(ScError::InvalidUrl("ссылка должна вести на soundcloud.com"));
    }
    if parsed.path == "/" {
        return Err(ScError::InvalidUrl("ссылка ведёт на главную, а не на трек"));
    }
    Ok(format!("{}{}", parsed.origin(), parsed.path))
}

pub(super) fn validate_https_url(
    url: &str,
    endpoints: &Endpoints,
    allow_download: bool,
) -> Result<ParsedUrl, ScError> {
    let parsed =
        parse_url(url).ok_or(ScError::InvalidUrl("не удалось разобрать адрес загрузки"))?;
    let http_ok = endpoints.allow_http && parsed.scheme == "http";
    if parsed.scheme != "https" && !http_ok {
        return Err(ScError::InvalidUrl("адрес загрузки должен быть https"));
    }
    let host_ok = if allow_download {
        is_allowed_download_host(&parsed.host)
    } else {
        is_soundcloud_host(&parsed.host)
    };
    if !host_ok {
        return Err(ScError::InvalidUrl("недопустимый хост загрузки"));
    }
    Ok(parsed)
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(super) fn extract_client_id(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(index) = rest.find("client_id") {
        let tail = &rest[index + "client_id".len()..];
        let start = tail
            .char_indices()
            .find(|(_, ch)| ch.is_ascii_alphanumeric())
            .map(|(index, _)| index)?;
        let id: String = tail[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric())
            .collect();
        if id.len() >= 20 {
            return Some(id);
        }
        rest = &tail[start..];
    }
    None
}

/// Script URLs from the SoundCloud page, limited to SoundCloud's own CDN.
pub(super) fn script_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut rest = html;
    while let Some(index) = rest.find("src=") {
        let tail = &rest[index + 4..];
        let quote = match tail.chars().next() {
            Some(ch @ ('"' | '\'')) => ch,
            _ => {
                rest = tail;
                continue;
            }
        };
        let tail = &tail[quote.len_utf8()..];
        let end = match tail.find(quote) {
            Some(end) => end,
            None => break,
        };
        let candidate = &tail[..end];
        if let Some(parsed) = parse_url(candidate) {
            if parsed.scheme == "https" && is_sndcdn_host(&parsed.host) {
                urls.push(candidate.to_string());
            }
        }
        rest = &tail[end..];
    }
    urls.sort();
    urls.dedup();
    urls.truncate(24);
    urls
}

// ─── curl plumbing ──────────────────────────────────────────

pub(super) fn protocol_args(endpoints: &Endpoints) -> Vec<String> {
    let proto = if endpoints.allow_http {
        "=https,http"
    } else {
        "=https"
    };
    vec![
        "--proto".into(),
        proto.into(),
        "--proto-redir".into(),
        proto.into(),
    ]
}

pub(super) fn header_config(headers: &[(String, String)]) -> String {
    let mut config = String::new();
    for (name, value) in headers {
        let value = value.replace('\\', "\\\\").replace('"', "\\\"");
        config.push_str(&format!("header = \"{name}: {value}\"\n"));
    }
    config
}

fn run_curl(mut command: Command, config: &str) -> Result<std::process::Output, ScError> {
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| ScError::Network(format!("не удалось запустить curl: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config.as_bytes())
            .map_err(|error| ScError::Network(format!("curl stdin: {error}")))?;
    }
    child
        .wait_with_output()
        .map_err(|error| ScError::Network(format!("curl: {error}")))
}

const HTTP_MARKER: &[u8] = b"\n__OWNPLAYER_HTTP__";

fn split_marker(bytes: &[u8]) -> Option<(&[u8], u16)> {
    let position = bytes
        .windows(HTTP_MARKER.len())
        .rposition(|window| window == HTTP_MARKER)?;
    let status = std::str::from_utf8(&bytes[position + HTTP_MARKER.len()..])
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some((&bytes[..position], status))
}

pub(super) fn http_bytes(
    url: &str,
    headers: &[(String, String)],
    endpoints: &Endpoints,
    max: u64,
) -> Result<(u16, Vec<u8>), ScError> {
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "15",
        "--max-time",
        "25",
        "--max-filesize",
        &max.to_string(),
        "--user-agent",
        USER_AGENT,
        "--output",
        "-",
        "--write-out",
        "\n__OWNPLAYER_HTTP__%{http_code}",
        "--config",
        "-",
    ]);
    if headers.iter().any(|(name, _)| name == "Authorization") {
        validate_api_url(url, endpoints)?;
    } else {
        command.args(["--location", "--max-redirs", &MAX_REDIRECTS.to_string()]);
    }
    command.args(protocol_args(endpoints));
    command.arg(url);
    let output = run_curl(command, &header_config(headers))?;
    let Some((body, status)) = split_marker(&output.stdout) else {
        return Err(curl_failure(output.status.code().unwrap_or(-1)));
    };
    if !output.status.success() {
        return Err(curl_failure(output.status.code().unwrap_or(-1)));
    }
    Ok((status, body.to_vec()))
}

pub(super) fn http_text(
    url: &str,
    headers: &[(String, String)],
    endpoints: &Endpoints,
    max: u64,
) -> Result<(u16, String), ScError> {
    let (status, body) = http_bytes(url, headers, endpoints, max)?;
    Ok((status, String::from_utf8_lossy(&body).to_string()))
}

pub(super) fn curl_failure(code: i32) -> ScError {
    let detail = match code {
        5 | 6 => "не удалось разрешить адрес",
        7 => "нет соединения",
        28 => "истекло время ожидания",
        35 | 60 => "ошибка TLS-соединения",
        47 => "слишком много перенаправлений",
        63 => "файл больше допустимого размера",
        _ => "curl завершился с ошибкой",
    };
    ScError::Network(format!("{detail} (код {code})"))
}

pub(super) fn map_status(status: u16, body: &[u8]) -> ScError {
    match status {
        401 | 403 => ScError::AuthRejected,
        404 | 410 => ScError::NotFound,
        429 => ScError::RateLimited,
        500..=599 => ScError::Network(format!("сервер ответил HTTP {status}")),
        _ => ScError::Http(status, body_preview(body)),
    }
}

pub(super) fn body_preview(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut cleaned: String = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() > 300 {
        cleaned = cleaned.chars().take(300).collect();
    }
    cleaned
}

// ─── SoundCloud API ─────────────────────────────────────────

pub(super) fn client_id(endpoints: &Endpoints) -> Result<String, ScError> {
    if let Some(id) = &endpoints.client_id {
        return Ok(id.clone());
    }
    static CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some(id) = cache.lock().ok().and_then(|guard| guard.clone()) {
        return Ok(id);
    }
    let (status, html) = http_text(
        &format!("{}/", endpoints.web),
        &[],
        endpoints,
        2 * 1024 * 1024,
    )?;
    if status != 200 {
        return Err(map_status(status, html.as_bytes()));
    }
    let id = match extract_client_id(&html) {
        Some(id) => id,
        None => {
            let mut found = None;
            for script in script_urls(&html) {
                let (status, body) = match http_text(&script, &[], endpoints, 6 * 1024 * 1024) {
                    Ok(response) => response,
                    Err(_) => continue,
                };
                if status == 200 {
                    if let Some(id) = extract_client_id(&body) {
                        found = Some(id);
                        break;
                    }
                }
            }
            found.ok_or_else(|| {
                ScError::Network("не удалось получить публичный client_id SoundCloud".into())
            })?
        }
    };
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(id.clone());
    }
    Ok(id)
}

pub(super) fn api_headers(token: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Accept".into(), "application/json".into()),
        ("Origin".into(), "https://soundcloud.com".into()),
        ("Referer".into(), "https://soundcloud.com/".into()),
    ];
    if let Some(token) = token {
        headers.push(("Authorization".into(), format!("OAuth {token}")));
    }
    headers
}

pub(super) fn resolve_short_link(url: &str, endpoints: &Endpoints) -> Result<String, ScError> {
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--location",
        "--max-redirs",
        &MAX_REDIRECTS.to_string(),
        "--connect-timeout",
        "15",
        "--max-time",
        "25",
        "--max-filesize",
        "2097152",
        "--user-agent",
        USER_AGENT,
        "--output",
        "/dev/null",
        "--write-out",
        "%{http_code} %{url_effective}",
    ]);
    command.args(protocol_args(endpoints));
    command.arg(url);
    let output = run_curl(command, "")?;
    if !output.status.success() {
        return Err(curl_failure(output.status.code().unwrap_or(-1)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (status, effective) = text
        .trim()
        .split_once(' ')
        .ok_or_else(|| ScError::Network("SoundCloud не вернул адрес трека".into()))?;
    let status: u16 = status.parse().unwrap_or(0);
    if status >= 400 {
        return Err(map_status(status, &[]));
    }
    let parsed =
        parse_url(effective).ok_or(ScError::InvalidUrl("короткая ссылка ведёт не на трек"))?;
    if !is_soundcloud_host(&parsed.host) {
        return Err(ScError::InvalidUrl(
            "короткая ссылка ведёт не на soundcloud.com",
        ));
    }
    normalize_track_url(effective, endpoints.allow_http)
}

pub(super) fn resolve_track_info(
    track_url: &str,
    endpoints: &Endpoints,
) -> Result<TrackInfo, ScError> {
    let cid = client_id(endpoints)?;
    let url = format!(
        "{}/resolve?url={}&client_id={}",
        endpoints.api.trim_end_matches('/'),
        urlencode(track_url),
        cid
    );
    let (status, body) = http_bytes(&url, &api_headers(None), endpoints, MAX_JSON)?;
    if status != 200 {
        return Err(map_status(status, &body));
    }
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| ScError::Network("SoundCloud вернул неожиданный ответ".into()))?;
    parse_track(&value, track_url)
}

pub(super) fn parse_track(value: &Value, fallback_url: &str) -> Result<TrackInfo, ScError> {
    if value.get("kind").and_then(Value::as_str) == Some("playlist")
        || value.get("kind").and_then(Value::as_str) == Some("user")
    {
        return Err(ScError::InvalidUrl("ссылка должна вести на отдельный трек"));
    }
    if value.get("policy").and_then(Value::as_str) == Some("BLOCK") {
        return Err(ScError::NotFound);
    }
    let id = value
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| ScError::Network("в ответе SoundCloud нет ID трека".into()))?;
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Без названия")
        .to_string();
    let artist = value
        .get("user")
        .and_then(|user| user.get("username"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|artist| !artist.is_empty())
        .unwrap_or("Неизвестный исполнитель")
        .to_string();
    let duration_ms = value
        .get("duration")
        .and_then(|duration| {
            duration
                .as_u64()
                .or_else(|| duration.as_f64().map(|value| value.round() as u64))
        })
        .unwrap_or(0);
    let download_url = value
        .get("download_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_string);
    Ok(TrackInfo {
        id,
        title,
        artist,
        artwork_url: value
            .get("artwork_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        duration_ms,
        downloadable: value.get("downloadable").and_then(Value::as_bool),
        has_downloads_left: value.get("has_downloads_left").and_then(Value::as_bool),
        permalink_url: value
            .get("permalink_url")
            .and_then(Value::as_str)
            .unwrap_or(fallback_url)
            .to_string(),
        genre: value
            .get("genre")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        created_at: value
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        download_url,
    })
}
