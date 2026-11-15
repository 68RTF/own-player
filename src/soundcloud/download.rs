use super::*;
use std::io::Read;
#[derive(Clone, Copy, Debug)]
enum AuthScheme {
    None,
    OAuth,
    Session,
}

fn download_headers(
    token: Option<&str>,
    scheme: AuthScheme,
    api_host: bool,
) -> Vec<(String, String)> {
    let mut headers = vec![("Accept".into(), "*/*".into())];
    if api_host {
        headers.push(("Referer".into(), "https://soundcloud.com/".into()));
        headers.push(("Origin".into(), "https://soundcloud.com".into()));
    }
    if let Some(token) = token {
        match scheme {
            AuthScheme::OAuth => headers.push(("Authorization".into(), format!("OAuth {token}"))),
            AuthScheme::Session => headers.push(("x-session-id".into(), token.to_string())),
            AuthScheme::None => {}
        }
    }
    headers
}

fn content_length(headers_path: &Path) -> Option<u64> {
    let text = fs::read_to_string(headers_path).ok()?;
    let mut total = None;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            total = value.trim().parse().ok();
        }
    }
    total
}

pub(super) fn redirect_target(
    url: &str,
    location: &str,
    endpoints: &Endpoints,
) -> Result<String, ScError> {
    let base = validate_https_url(url, endpoints, true)?;
    let target = if location.starts_with("//") {
        format!("{}:{location}", base.scheme)
    } else if location.starts_with('/') {
        format!("{}{location}", base.origin())
    } else {
        location.to_owned()
    };
    validate_https_url(&target, endpoints, true)?;
    Ok(target)
}

#[allow(clippy::too_many_arguments)]
fn download_attempt(
    url: &str,
    token: Option<&str>,
    scheme: AuthScheme,
    raw: &Path,
    headers_path: &Path,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
) -> Result<(), ScError> {
    let parsed = validate_https_url(url, endpoints, true)?;
    let api_host = parsed.host == "api-v2.soundcloud.com" || parsed.host == "api.soundcloud.com";
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "15",
        "--max-time",
        "1800",
        "--speed-time",
        "60",
        "--speed-limit",
        "1024",
        "--max-filesize",
        &MAX_AUDIO.to_string(),
        "--user-agent",
        USER_AGENT,
        "--output",
        &raw.to_string_lossy(),
        "--dump-header",
        &headers_path.to_string_lossy(),
        "--write-out",
        "\n__OWNPLAYER_DL__%{http_code}\n%{content_type}\n%{redirect_url}",
        "--config",
        "-",
    ]);
    command.args(protocol_args(endpoints));
    command.arg(url);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());
    let config = header_config(&download_headers(token, scheme, api_host));
    let mut child = command
        .spawn()
        .map_err(|error| ScError::Network(format!("не удалось запустить curl: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config.as_bytes())
            .map_err(|error| ScError::Network(format!("curl stdin: {error}")))?;
    }

    let mut last_size = u64::MAX;
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(raw);
            let _ = fs::remove_file(headers_path);
            return Err(ScError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(raw);
                let _ = fs::remove_file(headers_path);
                return Err(ScError::Io(error.to_string()));
            }
        }
        let size = fs::metadata(raw).map(|meta| meta.len()).unwrap_or(0);
        if size != last_size {
            last_size = size;
            let _ = tx.send(Event::Downloading {
                received: size,
                total: content_length(headers_path),
            });
        }
        std::thread::sleep(POLL);
    }

    let output = child
        .wait_with_output()
        .map_err(|error| ScError::Io(error.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(position) = stdout.rfind("__OWNPLAYER_DL__") else {
        let _ = fs::remove_file(raw);
        let _ = fs::remove_file(headers_path);
        return Err(curl_failure(output.status.code().unwrap_or(-1)));
    };
    let mut lines = stdout[position + "__OWNPLAYER_DL__".len()..].lines();
    let status: u16 = lines.next().unwrap_or("").trim().parse().unwrap_or(0);
    let content_type = lines.next().unwrap_or("").trim().to_ascii_lowercase();
    let location = lines.next().unwrap_or("").trim();

    if status == 0 || !output.status.success() {
        return Err(curl_failure(output.status.code().unwrap_or(-1)));
    }
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Err(ScError::Redirect(redirect_target(
            url, location, endpoints,
        )?));
    }
    if status == 401 && token.is_none() {
        let _ = fs::remove_file(raw);
        let _ = fs::remove_file(headers_path);
        return Err(ScError::AuthRequired);
    }
    if !(200..300).contains(&status) {
        let _ = fs::remove_file(raw);
        let _ = fs::remove_file(headers_path);
        return Err(map_status(status, &[]));
    }
    if content_type.contains("application/json") {
        let mut body = Vec::new();
        fs::File::open(raw)
            .and_then(|file| file.take(MAX_JSON + 1).read_to_end(&mut body))
            .map_err(|error| ScError::Io(error.to_string()))?;
        if body.len() as u64 > MAX_JSON {
            return Err(ScError::Unsupported("слишком большой ответ JSON".into()));
        }
        let _ = fs::remove_file(raw);
        let _ = fs::remove_file(headers_path);
        let value: Value = serde_json::from_slice(&body)
            .map_err(|_| ScError::Unsupported("неожиданный JSON вместо аудио".into()))?;
        let uri = value
            .get("redirectUri")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|uri| !uri.is_empty())
            .ok_or_else(|| ScError::Unsupported("в ответе SoundCloud нет ссылки на файл".into()))?;
        validate_https_url(uri, endpoints, true)?;
        return Err(ScError::Redirect(uri.to_string()));
    }
    if content_type.starts_with("text/") {
        let _ = fs::remove_file(raw);
        let _ = fs::remove_file(headers_path);
        return Err(ScError::Unsupported(
            "SoundCloud вернул страницу вместо аудио".into(),
        ));
    }
    let size = fs::metadata(raw)
        .map_err(|error| ScError::Io(error.to_string()))?
        .len();
    let _ = tx.send(Event::Downloading {
        received: size,
        total: Some(size),
    });
    Ok(())
}

fn fetch_original(
    url: &str,
    token: Option<&str>,
    raw: &Path,
    headers_path: &Path,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
) -> Result<(), ScError> {
    let parsed = validate_https_url(url, endpoints, true)?;
    let api_host = parsed.host == "api-v2.soundcloud.com" || parsed.host == "api.soundcloud.com";
    let mut url = url.to_string();
    let mut auth = if api_host { token } else { None };
    let mut scheme = if auth.is_some() {
        AuthScheme::OAuth
    } else {
        AuthScheme::None
    };
    for hop in 0..=MAX_REDIRECTS {
        let _ = fs::remove_file(raw);
        let _ = fs::remove_file(headers_path);
        let result = download_attempt(&url, auth, scheme, raw, headers_path, endpoints, cancel, tx);
        let result = match result {
            Err(ScError::AuthRejected) if matches!(scheme, AuthScheme::OAuth) => {
                let _ = fs::remove_file(raw);
                let _ = fs::remove_file(headers_path);
                download_attempt(
                    &url,
                    auth,
                    AuthScheme::Session,
                    raw,
                    headers_path,
                    endpoints,
                    cancel,
                    tx,
                )
            }
            result => result,
        };
        match result {
            Err(ScError::Redirect(target)) if hop < MAX_REDIRECTS => {
                url = target;
                auth = None;
                scheme = AuthScheme::None;
            }
            Err(ScError::Redirect(_)) => return Err(curl_failure(47)),
            result => return result,
        }
    }
    unreachable!("redirect loop always returns on its last iteration")
}

fn download_target(info: &TrackInfo, endpoints: &Endpoints) -> Result<String, ScError> {
    if let Some(url) = info.download_url.as_deref() {
        validate_https_url(url, endpoints, true)?;
        return Ok(url.to_string());
    }
    let cid = client_id(endpoints)?;
    Ok(format!(
        "{}/tracks/{}/download?client_id={}",
        endpoints.api.trim_end_matches('/'),
        info.id,
        cid
    ))
}

fn fetch_cover(
    url: &str,
    dir: &Path,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
) -> Option<PathBuf> {
    if cancel.load(Ordering::SeqCst) {
        return None;
    }
    let parsed = parse_url(url)?;
    if !is_sndcdn_host(&parsed.host) && !is_soundcloud_host(&parsed.host) {
        return None;
    }
    let url = if let Some(index) = url.rfind("-large") {
        format!(
            "{}-t500x500{}",
            &url[..index],
            &url[index + "-large".len()..]
        )
    } else {
        url.to_string()
    };
    let (status, bytes) = http_bytes(&url, &[], endpoints, MAX_COVER).ok()?;
    if status != 200 || bytes.is_empty() {
        return None;
    }
    let is_jpeg = bytes.starts_with(&[0xff, 0xd8, 0xff]);
    let is_png = bytes.starts_with(&[0x89, b'P', b'N', b'G']);
    let is_webp = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP");
    if !is_jpeg && !is_png && !is_webp {
        return None;
    }
    let raw = dir.join(format!("cover-{}.raw", nonce()));
    if fs::write(&raw, &bytes).is_err() {
        return None;
    }
    if is_jpeg {
        return Some(raw);
    }
    let jpeg = dir.join(format!("cover-{}.jpg", nonce()));
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&raw)
        .args([
            "-vf",
            "scale=900:900:force_original_aspect_ratio=decrease",
            "-frames:v",
            "1",
        ])
        .arg(&jpeg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    let _ = fs::remove_file(&raw);
    (status.success() && jpeg.is_file()).then_some(jpeg)
}

#[derive(Debug)]
pub(super) struct AudioProbe {
    pub(super) duration_ms: u64,
    pub(super) format: String,
}

pub(super) fn probe_audio(path: &Path) -> Result<AudioProbe, ScError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|error| ScError::Unsupported(format!("ffprobe недоступен: {error}")))?;
    if !output.status.success() {
        return Err(ScError::Unsupported(
            "не удалось прочитать аудиофайл".into(),
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| ScError::Unsupported("ffprobe вернул неожиданный ответ".into()))?;
    let has_audio = value
        .get("streams")
        .and_then(Value::as_array)
        .is_some_and(|streams| {
            streams
                .iter()
                .any(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio"))
        });
    if !has_audio {
        return Err(ScError::Unsupported("в файле нет аудиодорожки".into()));
    }
    let format = value
        .get("format")
        .and_then(|format| format.get("format_name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let duration = value
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(|duration| {
            duration
                .as_str()
                .and_then(|value| value.parse::<f64>().ok())
                .or_else(|| duration.as_f64())
        })
        .unwrap_or(0.0);
    if !duration.is_finite() || duration <= 0.0 {
        return Err(ScError::Unsupported(
            "не удалось определить длительность".into(),
        ));
    }
    Ok(AudioProbe {
        duration_ms: (duration * 1000.0).round() as u64,
        format,
    })
}

pub(super) fn sniff(path: &Path) -> Result<(), ScError> {
    let mut buffer = [0u8; 32];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut buffer))
        .map_err(|_| ScError::Io("не удалось прочитать временный файл".into()))?;
    let head = &buffer[..read];
    if head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"Rar!")
        || head.starts_with(b"7z\xBC\xAF")
    {
        return Err(ScError::Unsupported(
            "SoundCloud отдал архив; распакуй его вручную".into(),
        ));
    }
    let trimmed = head
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|index| &head[index..])
        .unwrap_or(head);
    if trimmed.starts_with(b"<") || trimmed.starts_with(b"{") || trimmed.starts_with(b"[") {
        return Err(ScError::Unsupported(
            "вместо аудио пришёл текст или JSON".into(),
        ));
    }
    Ok(())
}

pub(super) fn extension_for(probe: &AudioProbe) -> Result<&'static str, ScError> {
    let format = probe.format.to_ascii_lowercase();
    if format.contains("mp3") {
        Ok("mp3")
    } else if format.contains("flac") {
        Ok("flac")
    } else if format.contains("wav") {
        Ok("wav")
    } else if format.contains("aiff") {
        Ok("aiff")
    } else if format.contains("ogg") {
        Ok("ogg")
    } else if format.contains("mp4") || format.contains("m4a") || format.contains("mov") {
        Ok("m4a")
    } else if format.contains("aac") {
        Ok("aac")
    } else if format.contains("webm") || format.contains("matroska") {
        Ok("webm")
    } else {
        Err(ScError::Unsupported(format!(
            "неизвестный аудиоформат {format}"
        )))
    }
}

pub(super) fn sanitize_component(input: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in input.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
            space = false;
        } else if ch.is_whitespace() {
            if !out.is_empty() && !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(ch);
            space = false;
        }
    }
    let out = out.trim_matches([' ', '.']).to_string();
    if out.chars().count() > 120 {
        out.chars().take(120).collect()
    } else {
        out
    }
}

pub(super) fn build_stem(artist: &str, title: &str) -> String {
    let artist = sanitize_component(artist);
    let title = sanitize_component(title);
    let artist = if artist.is_empty() || artist == "Неизвестный исполнитель" {
        String::new()
    } else {
        artist
    };
    let title = if title.is_empty() {
        "track".to_string()
    } else {
        title
    };
    if artist.is_empty() {
        title
    } else {
        format!("{artist} - {title}")
    }
}

fn unique_path(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    for index in 2..=999 {
        let candidate = dir.join(format!("{stem} ({index}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem} (999).{extension}"))
}

/// A file with the same name and duration is the same track, even when the
/// download index was lost. This keeps the folder free of silent duplicates.
pub(super) fn existing_by_name(dir: &Path, info: &TrackInfo) -> Option<PathBuf> {
    let stem = build_stem(&info.artist, &info.title);
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let same_stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == stem);
        if !same_stem || !path.is_file() {
            continue;
        }
        let audio = matches!(
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "mp3" | "m4a" | "flac" | "wav" | "ogg" | "opus" | "aac" | "aiff" | "ape" | "oga"
        );
        if !audio {
            continue;
        }
        let Ok(probe) = probe_audio(&path) else {
            continue;
        };
        let duration_matches =
            info.duration_ms == 0 || probe.duration_ms.abs_diff(info.duration_ms) <= 2_000;
        if duration_matches {
            return Some(path);
        }
    }
    None
}

fn metadata_args(info: &TrackInfo) -> Vec<String> {
    let mut args = Vec::new();
    let mut push = |key: &str, value: &str| {
        let value = value.trim();
        if !value.is_empty() {
            args.push("-metadata".into());
            args.push(format!("{key}={value}"));
        }
    };
    push("title", &info.title);
    push("artist", &info.artist);
    push("genre", &info.genre);
    push("comment", &info.permalink_url);
    if info.created_at.len() >= 4 {
        push("date", &info.created_at[..4]);
    }
    args
}

fn run_ffmpeg(args: &[String]) -> bool {
    Command::new("ffmpeg")
        .args(["-v", "error", "-y"])
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(super) fn embed_metadata(
    raw: &Path,
    destination: &Path,
    info: &TrackInfo,
    cover: Option<&Path>,
    extension: &str,
    guard: &mut TempGuard,
) -> Result<bool, ScError> {
    // Keep the real extension last so ffmpeg can infer the output format.
    let out = destination.with_extension(format!("part.{extension}"));
    guard.add(out.clone());
    let tags = metadata_args(info);
    let cover_supported = matches!(extension, "mp3" | "m4a" | "flac" | "aiff");
    let mut cover_embedded = false;

    if let (Some(cover), true) = (cover, cover_supported) {
        let mut args = vec!["-i".into(), raw.to_string_lossy().into_owned()];
        args.extend(["-i".into(), cover.to_string_lossy().into_owned()]);
        args.extend([
            "-map".into(),
            "0:a".into(),
            "-map".into(),
            "1:v".into(),
            "-c:a".into(),
            "copy".into(),
            "-c:v".into(),
            "copy".into(),
            "-disposition:v:0".into(),
            "attached_pic".into(),
        ]);
        if extension == "m4a" {
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
        if extension == "mp3" {
            args.extend(["-id3v2_version".into(), "3".into()]);
            args.extend(["-metadata:s:v".into(), "comment=Cover (front)".into()]);
        }
        args.extend(tags.clone());
        args.push(out.to_string_lossy().into_owned());
        cover_embedded = run_ffmpeg(&args);
        if !cover_embedded {
            let _ = fs::remove_file(&out);
        }
    }

    if !cover_embedded {
        let mut args = vec![
            "-i".into(),
            raw.to_string_lossy().into_owned(),
            "-map".into(),
            "0:a".into(),
            "-c".into(),
            "copy".into(),
        ];
        if extension == "m4a" {
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
        if extension == "mp3" {
            args.extend(["-id3v2_version".into(), "3".into()]);
        }
        args.extend(tags);
        args.push(out.to_string_lossy().into_owned());
        if !run_ffmpeg(&args) {
            fs::copy(raw, &out).map_err(|error| ScError::Io(error.to_string()))?;
        }
    }

    fs::rename(&out, destination).map_err(|error| ScError::Io(error.to_string()))?;
    Ok(cover_embedded)
}

pub(super) fn write_sidecar_cover(cover: &Path, destination: &Path) {
    let sidecar = destination.with_extension("jpg");
    let temporary = sidecar.with_extension("jpg.part");
    if fs::copy(cover, &temporary).is_ok() && fs::rename(&temporary, &sidecar).is_err() {
        let _ = fs::remove_file(&temporary);
    }
}

pub(super) fn download_info(
    info: &TrackInfo,
    dir: &Path,
    token: Option<&str>,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
) -> Result<Outcome, ScError> {
    match info.downloadable {
        Some(true) => {}
        Some(false) => return Err(ScError::NotDownloadable),
        None => return Err(ScError::DownloadStatusUnknown),
    }
    if info.has_downloads_left == Some(false) {
        return Err(ScError::DownloadsExhausted);
    }
    if info.download_url.is_none() && token.is_none() {
        return Err(ScError::AuthRequired);
    }
    let _ = tx.send(Event::Meta {
        title: info.title.clone(),
        artist: info.artist.clone(),
    });

    ensure_dir(dir)?;
    let index = load_index();
    if let Some(path) = index.existing(info.id) {
        return Ok(Outcome {
            path,
            title: info.title.clone(),
            artist: info.artist.clone(),
            already: true,
        });
    }
    if let Some(path) = existing_by_name(dir, info) {
        return Ok(Outcome {
            path,
            title: info.title.clone(),
            artist: info.artist.clone(),
            already: true,
        });
    }

    let target = download_target(info, endpoints)?;
    let temporary = temp_dir(dir)?;
    let mut guard = TempGuard::new();
    let raw = temporary.join(format!("audio-{}.part", nonce()));
    let headers_path = temporary.join(format!("headers-{}.txt", nonce()));
    guard.add(raw.clone());
    guard.add(headers_path.clone());

    fetch_original(&target, token, &raw, &headers_path, endpoints, cancel, tx)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ScError::Cancelled);
    }

    let _ = tx.send(Event::Processing);
    sniff(&raw)?;
    let probe = probe_audio(&raw)?;
    if info.duration_ms > 0 && probe.duration_ms + 10_000 < info.duration_ms {
        return Err(ScError::Unsupported(
            "файл короче трека — вероятно, загрузка оборвалась".into(),
        ));
    }
    let extension = extension_for(&probe)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ScError::Cancelled);
    }
    let cover = info
        .artwork_url
        .as_deref()
        .and_then(|url| fetch_cover(url, &temporary, endpoints, cancel));
    if let Some(path) = &cover {
        guard.add(path.clone());
    }
    if cancel.load(Ordering::SeqCst) {
        return Err(ScError::Cancelled);
    }

    let stem = build_stem(&info.artist, &info.title);
    let destination = unique_path(dir, &stem, extension);
    let cover_embedded = embed_metadata(
        &raw,
        &destination,
        info,
        cover.as_deref(),
        extension,
        &mut guard,
    )?;
    if let Some(cover) = cover.as_deref() {
        if !cover_embedded {
            write_sidecar_cover(cover, &destination);
        }
    }

    let mut updated = load_index();
    if let Err(error) = remember(&mut updated, info, &destination) {
        eprintln!(
            "Не удалось записать индекс загрузок SoundCloud: {}",
            error.message()
        );
    }
    Ok(Outcome {
        path: destination,
        title: info.title.clone(),
        artist: info.artist.clone(),
        already: false,
    })
}
