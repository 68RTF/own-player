use super::*;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::sync::atomic::AtomicU64;
use std::time::{SystemTime, UNIX_EPOCH};

static NONCE: AtomicU64 = AtomicU64::new(0);
pub(crate) fn default_download_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let music = std::env::var_os("XDG_MUSIC_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            let home = home.as_ref()?;
            let config = home.join(".config/user-dirs.dirs");
            let text = fs::read_to_string(config).ok()?;
            for line in text.lines() {
                let line = line.trim();
                let Some(value) = line.strip_prefix("XDG_MUSIC_DIR=") else {
                    continue;
                };
                let value = value.trim().trim_matches('"');
                if value.is_empty() {
                    continue;
                }
                let expanded = value.replace("$HOME", &home.to_string_lossy());
                let path = PathBuf::from(expanded);
                if path.is_absolute() {
                    return Some(path);
                }
            }
            None
        })
        .or_else(|| home.as_ref().map(|home| home.join("Music")))
        .unwrap_or_else(|| PathBuf::from("Music"));
    music.join("SoundCloud")
}

pub(super) fn ensure_dir(path: &Path) -> Result<(), ScError> {
    fs::create_dir_all(path).map_err(|error| ScError::Io(error.to_string()))
}

pub(super) fn temp_dir(root: &Path) -> Result<PathBuf, ScError> {
    let path = root.join(".own-player-tmp");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&path)
        .map_err(|error| ScError::Io(error.to_string()))?;
    Ok(path)
}

pub(super) fn nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!(
        "{}-{}-{}",
        std::process::id(),
        nanos,
        NONCE.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) fn cleanup_stale_temp(root: &Path) {
    let Ok(entries) = fs::read_dir(root.join(".own-player-tmp")) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 60 * 60));
        if old && metadata.is_file() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub(super) struct TempGuard {
    paths: Vec<PathBuf>,
}

impl TempGuard {
    pub(super) fn new() -> Self {
        Self { paths: Vec::new() }
    }

    pub(super) fn add(&mut self, path: PathBuf) {
        self.paths.push(path);
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

// ─── duplicate index ────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexEntry {
    path: PathBuf,
    title: String,
    artist: String,
    url: String,
    bytes: u64,
    downloaded_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct DownloadIndex {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    tracks: BTreeMap<String, IndexEntry>,
}

fn default_version() -> u32 {
    1
}

impl Default for DownloadIndex {
    fn default() -> Self {
        Self {
            version: 1,
            tracks: BTreeMap::new(),
        }
    }
}

impl DownloadIndex {
    pub(super) fn existing(&self, id: u64) -> Option<PathBuf> {
        let entry = self.tracks.get(&id.to_string())?;
        let metadata = fs::metadata(&entry.path).ok()?;
        (metadata.is_file() && metadata.len() > 0).then(|| entry.path.clone())
    }
}

fn data_root() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(root.join("own-player"))
}

fn index_path() -> Option<PathBuf> {
    Some(data_root()?.join("soundcloud-downloads.json"))
}

pub(super) fn load_index() -> DownloadIndex {
    let Some(path) = index_path() else {
        return DownloadIndex::default();
    };
    let Ok(data) = fs::read(path) else {
        return DownloadIndex::default();
    };
    serde_json::from_slice(&data).unwrap_or_else(|error| {
        eprintln!("Не удалось прочитать индекс загрузок SoundCloud: {error}");
        DownloadIndex::default()
    })
}

fn save_index(index: &DownloadIndex) -> Result<(), ScError> {
    let path = index_path().ok_or_else(|| ScError::Io("не найден домашний каталог".into()))?;
    let parent = path
        .parent()
        .ok_or_else(|| ScError::Io("некорректный путь индекса".into()))?;
    fs::create_dir_all(parent).map_err(|error| ScError::Io(error.to_string()))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| ScError::Io(error.to_string()))?;
        serde_json::to_writer_pretty(&mut file, index)
            .map_err(|error| ScError::Io(error.to_string()))?;
        file.write_all(b"\n")
            .map_err(|error| ScError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| ScError::Io(error.to_string()))?;
        fs::rename(&temporary, &path).map_err(|error| ScError::Io(error.to_string()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn remember(
    index: &mut DownloadIndex,
    info: &TrackInfo,
    path: &Path,
) -> Result<(), ScError> {
    let bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let downloaded_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    index.tracks.insert(
        info.id.to_string(),
        IndexEntry {
            path: path.to_path_buf(),
            title: info.title.clone(),
            artist: info.artist.clone(),
            url: info.permalink_url.clone(),
            bytes,
            downloaded_at,
        },
    );
    save_index(index)
}
