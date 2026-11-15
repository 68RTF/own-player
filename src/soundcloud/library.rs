use super::*;

#[derive(Clone, Debug)]
pub(super) struct SkippedEntry {
    pub(super) url: String,
    pub(super) title: String,
    pub(super) artist: String,
    pub(super) reason: String,
}

#[derive(Default)]
pub(super) struct BatchSummary {
    pub(super) downloaded: usize,
    pub(super) already: usize,
    pub(super) failed: usize,
    pub(super) skipped: usize,
    pub(super) cancelled: bool,
    pub(super) report: Option<PathBuf>,
}

pub(super) fn validate_api_url(url: &str, endpoints: &Endpoints) -> Result<(), ScError> {
    let parsed = parse_url(url).ok_or(ScError::InvalidUrl("не удалось разобрать адрес API"))?;
    let http_ok = endpoints.allow_http && parsed.scheme == "http";
    if parsed.scheme != "https" && !http_ok {
        return Err(ScError::InvalidUrl("адрес API должен быть https"));
    }
    if !matches!(
        parsed.host.as_str(),
        "api-v2.soundcloud.com" | "api.soundcloud.com"
    ) {
        return Err(ScError::InvalidUrl("недопустимый хост API"));
    }
    Ok(())
}

pub(super) fn with_client_id(url: &str, cid: &str) -> String {
    if url.contains("client_id=") {
        url.to_string()
    } else if url.contains('?') {
        format!("{url}&client_id={cid}")
    } else {
        format!("{url}?client_id={cid}")
    }
}

fn api_json(
    url: &str,
    token: &str,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
) -> Result<Value, ScError> {
    for attempt in 0..4u64 {
        if cancel.load(Ordering::SeqCst) {
            return Err(ScError::Cancelled);
        }
        let (status, body) = http_bytes(url, &api_headers(Some(token)), endpoints, MAX_JSON)?;
        if status == 429 && attempt < 3 {
            for _ in 0..20 * (attempt + 1) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(ScError::Cancelled);
                }
                std::thread::sleep(POLL);
            }
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(map_status(status, &body));
        }
        return serde_json::from_slice(&body)
            .map_err(|_| ScError::Network("SoundCloud вернул неожиданный JSON".into()));
    }
    Err(ScError::RateLimited)
}

#[derive(Default)]
pub(super) struct LibraryTracks {
    pub(super) found: BTreeMap<u64, TrackInfo>,
    pub(super) skipped: BTreeMap<u64, SkippedEntry>,
    pub(super) pending: BTreeMap<u64, TrackInfo>,
}

impl LibraryTracks {
    pub(super) fn note_track(&mut self, value: &Value) {
        let Ok(info) = parse_track(value, "") else {
            return;
        };
        let id = info.id;
        if self.found.contains_key(&id) {
            return;
        }
        match info.downloadable {
            Some(true) if info.has_downloads_left != Some(false) => {
                self.pending.remove(&id);
                self.skipped.remove(&id);
                self.found.insert(id, info);
            }
            Some(allowed) => {
                self.pending.remove(&id);
                let reason = if allowed {
                    "лимит скачиваний исчерпан"
                } else {
                    "автор не включил скачивание"
                };
                self.skip(info, reason);
            }
            None if !self.skipped.contains_key(&id) => {
                self.pending.entry(id).or_insert(info);
            }
            None => {}
        }
    }

    fn skip(&mut self, info: TrackInfo, reason: &str) {
        self.skipped.entry(info.id).or_insert(SkippedEntry {
            url: info.permalink_url,
            title: info.title,
            artist: info.artist,
            reason: reason.to_string(),
        });
    }

    fn note_playlist(&mut self, value: &Value) {
        if let Some(tracks) = value.get("tracks").and_then(Value::as_array) {
            for track in tracks {
                self.note_track(track);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_pages<F>(
    mut url: String,
    token: &str,
    cid: &str,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    seen: &mut usize,
    mut visit: F,
) -> Result<(), ScError>
where
    F: FnMut(&Value),
{
    for _ in 0..500 {
        if cancel.load(Ordering::SeqCst) {
            return Err(ScError::Cancelled);
        }
        url = with_client_id(&url, cid);
        let page = api_json(&url, token, endpoints, cancel)?;
        let items = page
            .as_array()
            .or_else(|| page.get("collection").and_then(Value::as_array))
            .ok_or_else(|| ScError::Network("в ответе SoundCloud нет списка записей".into()))?;
        for item in items {
            *seen += 1;
            visit(item);
        }
        let _ = tx.send(Event::LibraryCount(*seen));
        let Some(next) = page.get("next_href").and_then(Value::as_str) else {
            return Ok(());
        };
        url = next.to_owned();
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(ScError::Unsupported(
        "слишком много страниц в библиотеке SoundCloud".into(),
    ))
}

pub(super) fn collect_library(
    token: &str,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
) -> Result<(Vec<TrackInfo>, Vec<SkippedEntry>), ScError> {
    let cid = client_id(endpoints)?;
    let api = endpoints.api.trim_end_matches('/');
    let me = api_json(
        &format!("{api}/me?client_id={cid}"),
        token,
        endpoints,
        cancel,
    )?;
    let uid = me
        .get("id")
        .and_then(Value::as_u64)
        .filter(|id| *id > 0)
        .ok_or_else(|| ScError::Network("не удалось определить пользователя SoundCloud".into()))?;

    let mut library = LibraryTracks::default();
    let mut seen = 0usize;
    let base = format!("{api}/users/{uid}");

    walk_pages(
        format!("{base}/tracks?limit=200&linked_partitioning=1"),
        token,
        &cid,
        endpoints,
        cancel,
        tx,
        &mut seen,
        |item| library.note_track(item),
    )?;
    walk_pages(
        format!("{base}/playlists?limit=100&linked_partitioning=1"),
        token,
        &cid,
        endpoints,
        cancel,
        tx,
        &mut seen,
        |item| library.note_playlist(item),
    )?;
    walk_pages(
        format!("{base}/likes?limit=200&linked_partitioning=1"),
        token,
        &cid,
        endpoints,
        cancel,
        tx,
        &mut seen,
        |item| {
            if let Some(track) = item.get("track") {
                library.note_track(track);
            } else if let Some(playlist) = item.get("playlist") {
                library.note_playlist(playlist);
            }
        },
    )?;
    // Liked playlists are not exposed on every account; ignore only "not found".
    if let Err(error) = walk_pages(
        format!("{base}/likes/playlists?limit=100&linked_partitioning=1"),
        token,
        &cid,
        endpoints,
        cancel,
        tx,
        &mut seen,
        |item| {
            if let Some(playlist) = item.get("playlist") {
                library.note_playlist(playlist);
            } else {
                library.note_playlist(item);
            }
        },
    ) {
        if !matches!(error, ScError::NotFound | ScError::Http(404, _)) {
            return Err(error);
        }
    }

    for info in std::mem::take(&mut library.pending).into_values() {
        let url = format!("{api}/tracks/{}?client_id={cid}", info.id);
        match api_json(&url, token, endpoints, cancel) {
            Ok(value)
                if value.get("downloadable").and_then(Value::as_bool).is_some()
                    && parse_track(&value, "").is_ok_and(|track| track.id == info.id) =>
            {
                library.note_track(&value);
            }
            Ok(_) => library.skip(info, "SoundCloud не сообщил, доступно ли скачивание"),
            Err(ScError::NotFound | ScError::Http(404, _)) => {
                library.skip(info, "трек недоступен в SoundCloud");
            }
            Err(error) => return Err(error),
        }
    }

    Ok((
        library.found.into_values().collect(),
        library.skipped.into_values().collect(),
    ))
}

pub(super) fn write_skipped_report(dir: &Path, entries: &[SkippedEntry]) -> Option<PathBuf> {
    if entries.is_empty() {
        return None;
    }
    let path = dir.join("soundcloud-skipped.txt");
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| {
        left.artist
            .to_lowercase()
            .cmp(&right.artist.to_lowercase())
            .then(left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    let mut text = String::from(
        "Свой плеер — пропущенные треки SoundCloud\n\
         Скачиваются только треки, которые авторы разрешили скачивать.\n\n",
    );
    for entry in &sorted {
        text.push_str(&format!(
            "[{}] {} — {}\n{}\n\n",
            entry.reason,
            entry.artist.replace(['\n', '\r'], " "),
            entry.title.replace(['\n', '\r'], " "),
            entry.url
        ));
    }
    let mut guard = TempGuard::new();
    let staged = temp_dir(dir).ok()?.join(format!("report-{}.part", nonce()));
    guard.add(staged.clone());
    fs::write(&staged, text).ok()?;
    fs::rename(staged, &path).ok()?;
    Some(path)
}

pub(super) fn run_library(
    dir: &Path,
    token: &str,
    endpoints: &Endpoints,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
    summary: &mut BatchSummary,
) -> Result<(), ScError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(ScError::Cancelled);
    }
    let (tracks, mut skipped) = collect_library(token, endpoints, cancel, tx)?;
    summary.skipped = skipped.len();
    let _ = tx.send(Event::BatchStarted {
        total: tracks.len(),
        skipped: skipped.len(),
    });
    ensure_dir(dir)?;
    for (position, info) in tracks.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            summary.cancelled = true;
            break;
        }
        let _ = tx.send(Event::BatchItem {
            index: position + 1,
            total: tracks.len(),
            title: info.title.clone(),
            artist: info.artist.clone(),
        });
        match download_info(info, dir, Some(token), endpoints, cancel, tx) {
            Ok(outcome) => {
                if outcome.already {
                    summary.already += 1;
                } else {
                    summary.downloaded += 1;
                }
                let _ = tx.send(Event::Done(outcome));
            }
            Err(ScError::Cancelled) => {
                summary.cancelled = true;
                break;
            }
            Err(error) => {
                if matches!(
                    error,
                    ScError::NotDownloadable | ScError::DownloadsExhausted
                ) {
                    summary.skipped += 1;
                } else {
                    summary.failed += 1;
                }
                skipped.push(SkippedEntry {
                    url: info.permalink_url.clone(),
                    title: info.title.clone(),
                    artist: info.artist.clone(),
                    reason: error.message(),
                });
            }
        }
    }
    summary.report = write_skipped_report(dir, &skipped);
    Ok(())
}
