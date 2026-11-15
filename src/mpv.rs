use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) struct Mpv {
    child: Child,
    socket: UnixStream,
    socket_path: PathBuf,
    buffer: Vec<u8>,
    req: u64,
    outgoing: VecDeque<u8>,
    awaiting_start: usize,
    pub(crate) pos: f64,
    pub(crate) duration: f64,
    pub(crate) paused: bool,
    pub(crate) loaded: bool,
    pub(crate) paused_lyric_position: Option<f64>,
    pub(crate) ended: bool,
    pub(crate) failed: bool,
    pub(crate) error: Option<String>,
}

impl Mpv {
    pub(crate) fn spawn() -> Option<(Child, PathBuf)> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("own-player-{}-{nonce}", std::process::id()));
        fs::DirBuilder::new().mode(0o700).create(&directory).ok()?;
        let path = directory.join("mpv.sock");
        let mut command = Command::new("mpv");
        command
            .args([
                "--no-config",
                "--idle=yes",
                "--no-video",
                "--no-terminal",
                "--audio-display=no",
                "--force-window=no",
                &format!("--input-ipc-server={}", path.display()),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(test)]
        command.arg("--ao=null");
        let parent_pid = std::process::id() as libc::pid_t;
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent_pid {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        match command.spawn() {
            Ok(child) => Some((child, path)),
            Err(_) => {
                let _ = fs::remove_dir(directory);
                None
            }
        }
    }

    pub(crate) fn connect(mut child: Child, path: PathBuf) -> Option<Self> {
        for _ in 0..30 {
            if let Ok(socket) = UnixStream::connect(&path) {
                if socket.set_nonblocking(true).is_err() {
                    break;
                }
                let mut player = Self {
                    child,
                    socket,
                    socket_path: path,
                    buffer: Vec::new(),
                    req: 0,
                    outgoing: VecDeque::new(),
                    awaiting_start: 0,
                    pos: 0.0,
                    duration: 0.0,
                    paused: true,
                    loaded: false,
                    paused_lyric_position: None,
                    ended: false,
                    failed: false,
                    error: None,
                };
                for property in ["time-pos", "duration", "pause", "volume"] {
                    player.req += 1;
                    let _ = player.command(json!(["observe_property", player.req, property]));
                }
                return Some(player);
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&path);
        if let Some(directory) = path.parent() {
            let _ = fs::remove_dir(directory);
        }
        None
    }

    pub(crate) fn command(&mut self, command: Value) -> std::io::Result<()> {
        self.req += 1;
        let is_load = command[0] == "loadfile";
        if is_load || matches!(command[0].as_str(), Some("seek" | "cycle" | "stop")) {
            self.paused_lyric_position = None;
        }
        let object = json!({"command": command, "request_id": self.req});
        self.outgoing.extend(format!("{object}\n").bytes());
        self.flush()?;
        if is_load {
            self.awaiting_start += 1;
        }
        Ok(())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        while !self.outgoing.is_empty() {
            match self.socket.write(self.outgoing.as_slices().0) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(written) => {
                    self.outgoing.drain(..written);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub(crate) fn read_events(&mut self) {
        if let Err(error) = self.flush() {
            self.error = Some(format!("Не удалось отправить команду mpv: {error}"));
            self.loaded = false;
            self.paused = true;
        }
        let mut chunk = [0u8; 4096];
        loop {
            match self.socket.read(&mut chunk) {
                Ok(0) => {
                    self.loaded = false;
                    self.paused = true;
                    if self.error.is_none() {
                        self.error = Some("Связь с mpv прервалась. Перезапусти плеер.".into());
                    }
                    break;
                }
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                match value["name"].as_str().unwrap_or("") {
                    "time-pos" => self.pos = value["data"].as_f64().unwrap_or(self.pos),
                    "duration" => self.duration = value["data"].as_f64().unwrap_or(self.duration),
                    "pause" => self.paused = value["data"].as_bool().unwrap_or(self.paused),
                    _ => {}
                }
                let event = value["event"].as_str();
                if event == Some("start-file") {
                    self.awaiting_start = self.awaiting_start.saturating_sub(1);
                }
                if event == Some("end-file") && self.awaiting_start == 0 {
                    match value["reason"].as_str() {
                        Some("eof") => {
                            self.ended = true;
                            self.loaded = false;
                        }
                        Some("error") => {
                            self.failed = true;
                            self.loaded = false;
                            self.paused = true;
                            self.error = Some("Не удалось воспроизвести файл. Проверь, что он доступен и не повреждён.".into());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        let _ = self.command(json!(["quit"]));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket_path);
        if let Some(directory) = self.socket_path.parent() {
            let _ = fs::remove_dir(directory);
        }
    }
}
