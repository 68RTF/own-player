use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) struct Mpv {
    child: Child,
    socket: UnixStream,
    pub(crate) event_source: Option<glib::SourceId>,
    pub(crate) write_source: Option<glib::SourceId>,
    socket_path: PathBuf,
    buffer: Vec<u8>,
    req: u64,
    outgoing: VecDeque<u8>,
    awaiting_start: VecDeque<u64>,
    pos: f64,
    sampled_at: Instant,
    pub(crate) duration: f64,
    pub(crate) generation: u64,
    pub(crate) volume: Option<f64>,
    pub(crate) muted: Option<bool>,
    volume_request: Option<u64>,
    mute_request: Option<u64>,
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
                    event_source: None,
                    write_source: None,
                    socket_path: path,
                    buffer: Vec::new(),
                    req: 0,
                    outgoing: VecDeque::new(),
                    awaiting_start: VecDeque::new(),
                    pos: 0.0,
                    sampled_at: Instant::now(),
                    duration: 0.0,
                    generation: 0,
                    volume: None,
                    muted: None,
                    volume_request: None,
                    mute_request: None,
                    paused: true,
                    loaded: false,
                    paused_lyric_position: None,
                    ended: false,
                    failed: false,
                    error: None,
                };
                for property in ["time-pos", "duration", "pause", "volume", "mute"] {
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

    pub(crate) fn socket_fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    pub(crate) fn has_outgoing(&self) -> bool {
        !self.outgoing.is_empty()
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
            self.awaiting_start.push_back(self.req);
        }
        Ok(())
    }

    pub(crate) fn set_volume(&mut self, volume: f64) -> std::io::Result<()> {
        self.command(json!(["set_property", "volume", volume]))?;
        // Read back the actual property after all earlier writes on this socket.
        self.command(json!(["get_property", "volume"]))?;
        self.volume_request = Some(self.req);
        Ok(())
    }

    pub(crate) fn set_muted(&mut self, muted: bool) -> std::io::Result<()> {
        self.command(json!(["set_property", "mute", muted]))?;
        self.command(json!(["get_property", "mute"]))?;
        self.mute_request = Some(self.req);
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

    pub(crate) fn set_position(&mut self, position: f64) {
        self.pos = position;
        self.sampled_at = Instant::now();
    }

    pub(crate) fn position(&self, speed: f64) -> f64 {
        if self.paused || !self.loaded {
            return self.paused_lyric_position.unwrap_or(self.pos);
        }
        let elapsed = self.sampled_at.elapsed().min(Duration::from_millis(120));
        if self.duration > 0.0 {
            (self.pos + elapsed.as_secs_f64() * speed).min(self.duration)
        } else {
            self.pos
        }
    }

    pub(crate) fn read_events(&mut self) -> std::io::Result<()> {
        self.flush()?;
        let mut chunk = [0u8; 4096];
        loop {
            match self.socket.read(&mut chunk) {
                Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        let mut buffer = std::mem::take(&mut self.buffer);
        let mut consumed = 0;
        for line in buffer.split_inclusive(|byte| *byte == b'\n') {
            if !line.ends_with(b"\n") {
                break;
            }
            consumed += line.len();
            if let Ok(value) = serde_json::from_slice::<Value>(line) {
                self.handle_event(&value);
            }
        }
        buffer.drain(..consumed);
        self.buffer = buffer;
        Ok(())
    }

    fn handle_event(&mut self, value: &Value) {
        let request = value["request_id"].as_u64();
        if request.is_some() && request == self.volume_request {
            self.volume_request = None;
            self.volume = value["data"].as_f64();
        }
        if request.is_some() && request == self.mute_request {
            self.mute_request = None;
            self.muted = value["data"].as_bool();
        }
        match value["name"].as_str().unwrap_or("") {
            "volume" if self.volume_request.is_none() => self.volume = value["data"].as_f64(),
            "mute" if self.mute_request.is_none() => self.muted = value["data"].as_bool(),
            "time-pos" => {
                if let Some(position) = value["data"].as_f64() {
                    self.set_position(position);
                }
            }
            "duration" => self.duration = value["data"].as_f64().unwrap_or(self.duration),
            "pause" => {
                if let Some(paused) = value["data"].as_bool() {
                    self.paused = paused;
                    self.sampled_at = Instant::now();
                }
            }
            _ => {}
        }
        match value["event"].as_str() {
            Some("start-file") => {
                self.awaiting_start.pop_front();
            }
            Some("end-file") if self.awaiting_start.is_empty() => match value["reason"].as_str() {
                Some("eof") => {
                    self.ended = true;
                    self.loaded = false;
                }
                Some("error") => self.playback_failed(),
                _ => {}
            },
            _ => {}
        }
        if value["error"]
            .as_str()
            .is_some_and(|error| error != "success")
        {
            if let Some(request) = value["request_id"].as_u64() {
                let failed_latest = self.awaiting_start.back() == Some(&request);
                self.awaiting_start.retain(|&pending| pending != request);
                if failed_latest {
                    self.playback_failed();
                }
            }
        }
    }

    fn playback_failed(&mut self) {
        self.failed = true;
        self.loaded = false;
        self.paused = true;
        self.error =
            Some("Не удалось воспроизвести файл. Проверь, что он доступен и не повреждён.".into());
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        if let Some(source) = self.event_source.take() {
            source.remove();
        }
        if let Some(source) = self.write_source.take() {
            source.remove();
        }
        let _ = self.command(json!(["quit"]));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.socket_path);
        if let Some(directory) = self.socket_path.parent() {
            let _ = fs::remove_dir(directory);
        }
    }
}
