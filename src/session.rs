use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, OnceLock};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Session {
    #[serde(default)]
    pub appearance: Appearance,
    #[serde(default)]
    pub muted: bool,
    pub paths: Vec<PathBuf>,
    pub current_path: Option<PathBuf>,
    #[serde(default)]
    pub play_next: Option<PathBuf>,
    pub shuffle: bool,
    pub repeat: usize,
    #[serde(default = "default_online_covers")]
    pub online_covers: bool,
    #[serde(default = "default_volume")]
    pub volume: f64,
    #[serde(default = "default_speed")]
    pub speed: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Appearance {
    pub dynamic_background: bool,
    pub animations: bool,
    pub expanded_sidebar: bool,
    pub lyrics_seek_hint_seen: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            dynamic_background: true,
            animations: true,
            expanded_sidebar: false,
            lyrics_seek_hint_seen: false,
        }
    }
}

fn default_online_covers() -> bool {
    true
}

fn default_volume() -> f64 {
    75.0
}

fn default_speed() -> f64 {
    1.0
}

impl Default for Session {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            muted: false,
            paths: Vec::new(),
            current_path: None,
            play_next: None,
            shuffle: false,
            repeat: 0,
            online_covers: true,
            volume: default_volume(),
            speed: default_speed(),
        }
    }
}

fn location() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
        })?;
    Some(root.join("own-player/session.json"))
}

pub(crate) fn load() -> io::Result<Session> {
    let path = location()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Не найден домашний каталог"))?;
    read_from(&path)
}

enum WriteRequest {
    Save(PathBuf, Session),
    Flush(mpsc::Sender<()>),
}

static WRITER: OnceLock<mpsc::Sender<WriteRequest>> = OnceLock::new();

pub(crate) fn save(session: Session) -> io::Result<()> {
    let path = location()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Не найден домашний каталог"))?;
    let sender = WRITER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || write_sessions(&receiver));
        sender
    });
    sender
        .send(WriteRequest::Save(path, session))
        .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))
}

pub(crate) fn flush() {
    if let Some(writer) = WRITER.get() {
        let (sender, receiver) = mpsc::channel();
        if writer.send(WriteRequest::Flush(sender)).is_ok() {
            let _ = receiver.recv();
        }
    }
}

fn write_sessions(receiver: &mpsc::Receiver<WriteRequest>) {
    let mut next = None;
    while let Some(request) = next.take().or_else(|| receiver.recv().ok()) {
        match request {
            WriteRequest::Save(path, mut session) => {
                while let Ok(request) = receiver.try_recv() {
                    match request {
                        WriteRequest::Save(new_path, new_session) if new_path == path => {
                            session = new_session;
                        }
                        other => {
                            next = Some(other);
                            break;
                        }
                    }
                }
                if let Err(error) = write_to(&path, &session) {
                    eprintln!("Не удалось сохранить очередь: {error}");
                }
            }
            WriteRequest::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn read_from(path: &Path) -> io::Result<Session> {
    let contents = fs::read(path)?;
    serde_json::from_slice(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_to(path: &Path, session: &Session) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Некорректный путь сессии"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, session)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{read_from, write_to, Session};
    use std::path::PathBuf;

    #[test]
    fn round_trip_preserves_queue_order_and_selection() {
        let directory =
            std::env::temp_dir().join(format!("own-player-session-test-{}", std::process::id()));
        let path = directory.join("session.json");
        let data = Session {
            paths: vec![PathBuf::from("/music/b.mp3"), PathBuf::from("/music/a.mp3")],
            current_path: Some(PathBuf::from("/music/a.mp3")),
            play_next: Some(PathBuf::from("/music/b.mp3")),
            shuffle: true,
            repeat: 2,
            online_covers: true,
            volume: 62.0,
            speed: 1.25,
            muted: true,
            appearance: super::Appearance {
                dynamic_background: false,
                animations: false,
                expanded_sidebar: true,
                lyrics_seek_hint_seen: true,
            },
        };
        write_to(&path, &data).unwrap();
        let restored = read_from(&path).unwrap();
        assert_eq!(restored.paths, data.paths);
        assert_eq!(restored.current_path, data.current_path);
        assert_eq!(restored.play_next, data.play_next);
        assert!(restored.shuffle);
        assert_eq!(restored.repeat, 2);
        assert!(restored.muted);
        assert!(!restored.appearance.dynamic_background);
        assert!(!restored.appearance.animations);
        assert!(restored.appearance.expanded_sidebar);
        assert!(restored.appearance.lyrics_seek_hint_seen);
        assert!(restored.online_covers);
        assert!((restored.volume - 62.0).abs() < f64::EPSILON);
        assert!((restored.speed - 1.25).abs() < f64::EPSILON);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn reads_queues_saved_with_download_settings() {
        let old = r#"{"paths":["/music/song.mp3"],"current_path":"/music/song.mp3","shuffle":false,"repeat":0,"soundcloud_dir":"/music/downloads"}"#;
        let restored: Session = serde_json::from_str(old).unwrap();

        assert_eq!(restored.paths, [PathBuf::from("/music/song.mp3")]);
        assert_eq!(
            restored.current_path,
            Some(PathBuf::from("/music/song.mp3"))
        );
        assert!(serde_json::to_value(restored)
            .unwrap()
            .get("soundcloud_dir")
            .is_none());
    }

    #[test]
    fn old_session_enables_online_covers_by_default() {
        let old = r#"{"paths":[],"current_path":null,"shuffle":false,"repeat":0}"#;
        let restored: Session = serde_json::from_str(old).unwrap();
        assert!(restored.online_covers);
        assert!((restored.volume - 75.0).abs() < f64::EPSILON);
        assert!((restored.speed - 1.0).abs() < f64::EPSILON);
    }
}
