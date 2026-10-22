use koe::{
    config::{Backend, Config, OutputMode},
    ipc,
    protocol::{Action, Phase, Request, Response},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixListener, UnixStream},
};

struct Harness {
    _dir: tempfile::TempDir,
    child: Child,
    socket: PathBuf,
    config: PathBuf,
    bin: PathBuf,
    clipboard: PathBuf,
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn script(path: &Path, body: &str) {
    let shell = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("sh"))
        .find(|p| p.is_file())
        .unwrap();
    fs::write(path, format!("#!{}\n{body}\n", shell.display())).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}
impl Harness {
    async fn start(endpoint: String, desktop: Option<&Path>) -> Self {
        Self::configured(endpoint, desktop, |_| {}).await
    }
    async fn configured(
        endpoint: String,
        desktop: Option<&Path>,
        configure: impl FnOnce(&mut Config),
    ) -> Self {
        Self::with_shell(endpoint, desktop, None, configure).await
    }
    async fn with_shell(
        endpoint: String,
        desktop: Option<&Path>,
        shell: Option<&Path>,
        configure: impl FnOnce(&mut Config),
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let raw = dir.path().join("samples.raw");
        let samples: Vec<u8> = (0..16000)
            .flat_map(|i| if i % 2 == 0 { 5000_i16 } else { -5000_i16 }.to_ne_bytes())
            .collect();
        fs::write(&raw, samples).unwrap();
        script(
            &bin.join("pw-record"),
            "cat \"$KOE_TEST_PCM\"\nexec sleep 60",
        );
        script(&bin.join("wl-copy"), "cat > \"$KOE_TEST_CLIPBOARD\"");
        let clipboard = dir.path().join("clipboard.txt");
        let config_path = dir.path().join("config.toml");
        let mut config = Config::default();
        config.engine.backend = Backend::Http;
        config.tray.enabled = false;
        config.engine.http.endpoint = endpoint;
        config.output.mode = if desktop.is_some() {
            OutputMode::Paste
        } else {
            OutputMode::None
        };
        config.output.notify = false;
        config.output.paste_delay_ms = 10;
        config.replacements.push(koe::config::Replacement {
            from: "seka".into(),
            to: "Sekka".into(),
        });
        configure(&mut config);
        fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
        let socket = dir.path().join("runtime/control.sock");
        let mut command = Command::new(env!("CARGO_BIN_EXE_koe"));
        command
            .args(["--config"])
            .arg(&config_path)
            .arg("--socket")
            .arg(&socket)
            .arg("daemon")
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("KOE_TEST_PCM", raw)
            .env("KOE_TEST_CLIPBOARD", &clipboard)
            .env_remove("AIZOME_SOCKET")
            .env_remove("AIZOME_SHELL_SOCKET")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(socket) = desktop {
            command.env("AIZOME_SOCKET", socket);
        }
        if let Some(socket) = shell {
            command.env("AIZOME_SHELL_SOCKET", socket);
        }
        let child = command.spawn().unwrap();
        let harness = Self {
            _dir: dir,
            child,
            socket,
            config: config_path,
            bin,
            clipboard,
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !harness.socket.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        harness
    }
    async fn call(&self, action: Action) -> Response {
        ipc::request(&self.socket, &Request::new(action))
            .await
            .unwrap()
    }
    async fn wait_for(&self, phase: Phase) -> Response {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let r = self.call(Action::Status).await;
                if r.status.phase == phase {
                    return r;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("never reached {phase:?}"))
    }
    async fn record_audio(&self) {
        assert!(self.call(Action::Start).await.ok);
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.call(Action::Status).await.status.elapsed_ms < 1000 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
}
async fn http_server(
    delay: Duration,
    response_body: &'static str,
) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        let mut buffer = [0; 4096];
        loop {
            let n = stream.read(&mut buffer).await.unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..n]);
            if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&request[..end]).to_lowercase();
                let len: usize = header
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if request.len() >= end + 4 + len {
                    break;
                }
            }
        }
        tokio::time::sleep(delay).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        let _ = stream.write_all(response.as_bytes()).await;
        request
    });
    (format!("http://{addr}/inference"), task)
}
#[tokio::test]
async fn complete_dictation_pause_resume_last_and_shutdown() {
    let (endpoint, server) = http_server(Duration::ZERO, r#"{"text":" hello seka 日本語 "}"#).await;
    let mut h = Harness::start(endpoint, None).await;
    let mut stream = UnixStream::connect(&h.socket).await.unwrap();
    ipc::write_json(&mut stream, &Request::new(Action::Subscribe))
        .await
        .unwrap();
    let mut events = BufReader::new(stream);
    assert!(ipc::read_line(&mut events).await.unwrap().is_some());
    h.record_audio().await;
    assert!(!h.call(Action::Start).await.ok);
    assert_eq!(h.call(Action::Pause).await.status.phase, Phase::Paused);
    assert_eq!(h.call(Action::Resume).await.status.phase, Phase::Recording);
    assert!(h.call(Action::Stop).await.ok);
    let done = h.wait_for(Phase::Idle).await;
    assert!(done.status.has_last);
    assert_eq!(
        h.call(Action::Last).await.text.unwrap(),
        "hello Sekka 日本語"
    );
    let request = server.await.unwrap();
    assert!(request.windows(4).any(|w| w == b"RIFF"));
    assert!(h.call(Action::ClearLast).await.ok);
    assert!(!h.call(Action::Last).await.ok);
    assert!(h.call(Action::Shutdown).await.ok);
    tokio::time::timeout(Duration::from_secs(3), async {
        while h.child.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!h.socket.exists());
}
#[tokio::test]
async fn cancellation_prevents_late_results() {
    let (endpoint, server) = http_server(
        Duration::from_millis(350),
        r#"{"text":"must never arrive"}"#,
    )
    .await;
    let h = Harness::start(endpoint, None).await;
    h.record_audio().await;
    h.call(Action::Stop).await;
    h.wait_for(Phase::Transcribing).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.call(Action::Cancel).await.status.phase, Phase::Idle);
    tokio::time::sleep(Duration::from_millis(450)).await;
    assert!(!h.call(Action::Status).await.status.has_last);
    assert!(!h.call(Action::Last).await.ok);
    server.abort();
    h.record_audio().await;
    assert!(h.call(Action::Cancel).await.ok);
}
#[tokio::test]
async fn protocol_limits_single_instance_and_stale_socket() {
    let h = Harness::start("http://127.0.0.1:9/inference".into(), None).await;
    assert_eq!(
        fs::metadata(&h.socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let second = Command::new(env!("CARGO_BIN_EXE_koe"))
        .arg("--config")
        .arg(&h.config)
        .arg("--socket")
        .arg(&h.socket)
        .arg("daemon")
        .output()
        .unwrap();
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));
    let response = ipc::request(
        &h.socket,
        &Request {
            action: Action::Status,
            version: 999,
        },
    )
    .await
    .unwrap();
    assert!(!response.ok);
    let mut bad = UnixStream::connect(&h.socket).await.unwrap();
    bad.write_all(&vec![b'x'; 65537]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(h.call(Action::Status).await.ok);
    assert!(!h.call(Action::Stop).await.ok);
    let status = h.call(Action::Status).await.status;
    assert_eq!(
        status.sequence,
        h.call(Action::Status).await.status.sequence
    );
}
#[tokio::test]
async fn microphone_failure_recovers_without_wedging() {
    let h = Harness::start("http://127.0.0.1:9/inference".into(), None).await;
    script(&h.bin.join("pw-record"), "exit 1");
    assert!(h.call(Action::Start).await.ok);
    let error = h.wait_for(Phase::Error).await;
    assert!(error.status.error.unwrap().contains("pw-record stopped"));
    assert_eq!(h.call(Action::Cancel).await.status.phase, Phase::Idle);
}
#[tokio::test]
async fn aizome_pastes_terminal_text_and_checks_focus() {
    for change_focus in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("compositor.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let history = calls.clone();
        let compositor = tokio::spawn(async move {
            let mut focus_reads = 0;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let line = ipc::read_line(&mut BufReader::new(&mut stream))
                    .await
                    .unwrap()
                    .unwrap();
                let value: serde_json::Value = serde_json::from_slice(&line).unwrap();
                history.lock().unwrap().push(value.clone());
                let response = match value["request"].as_str().unwrap() {
                    "focused-window" => {
                        focus_reads += 1;
                        serde_json::json!({"response":"window","window":{"id":if change_focus && focus_reads > 1 { 8 } else { 7 }, "app_id":"foot", "kit_app":false}})
                    }
                    "input-state" => {
                        serde_json::json!({"response":"input-state","state":{"pressed_keys":[]}})
                    }
                    "recovery-state" => {
                        serde_json::json!({"response":"recovery","state":{"locked":false}})
                    }
                    "synthesize" => serde_json::json!({"response":"ok"}),
                    other => panic!("unexpected request {other}"),
                };
                ipc::write_json(&mut stream, &response).await.unwrap();
            }
        });
        let (endpoint, _) = http_server(Duration::ZERO, r#"{"text":"dictated text"}"#).await;
        let h = Harness::start(endpoint, Some(&socket)).await;
        h.record_audio().await;
        h.call(Action::Stop).await;
        h.wait_for(if change_focus {
            Phase::Error
        } else {
            Phase::Idle
        })
        .await;
        let calls = calls.lock().unwrap();
        let paste = calls.iter().find(|v| v["request"] == "synthesize");
        if change_focus {
            assert!(paste.is_none());
            assert!(!h.clipboard.exists());
        } else {
            assert_eq!(paste.unwrap()["events"][0]["keys"], "ctrl+shift+v");
            assert_eq!(fs::read_to_string(&h.clipboard).unwrap(), "dictated text");
        }
        compositor.abort();
    }
}

#[tokio::test]
async fn automatic_submission_on_duration_and_silence() {
    for silence in [false, true] {
        let (endpoint, server) =
            http_server(Duration::from_millis(50), r#"{"text":"automatic stop"}"#).await;
        let h = Harness::configured(endpoint, None, |c| {
            if silence {
                c.audio.silence_seconds = 0.2;
            } else {
                c.audio.max_seconds = 1;
            }
        })
        .await;
        if silence {
            let pcm: Vec<u8> = (0..16000)
                .flat_map(|i| {
                    let sample: i16 = if i < 4800 { 5000 } else { 0 };
                    sample.to_ne_bytes()
                })
                .collect();
            fs::write(h._dir.path().join("samples.raw"), pcm).unwrap();
        }
        assert!(h.call(Action::Start).await.ok);
        let done = h.wait_for(Phase::Idle).await;
        assert!(done.status.has_last);
        assert!(done.status.elapsed_ms <= 1000);
        assert_eq!(h.call(Action::Last).await.text.unwrap(), "automatic stop");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn malformed_server_response_is_reported_without_echoing_private_text() {
    let (endpoint, server) =
        http_server(Duration::ZERO, r#"{"private":"secret speech not to log"}"#).await;
    let h = Harness::start(endpoint, None).await;
    h.record_audio().await;
    h.call(Action::Stop).await;
    let response = h.wait_for(Phase::Error).await;
    let error = response.status.error.unwrap();
    assert!(error.contains("text field"));
    assert!(!error.contains("secret speech"));
    assert!(h.call(Action::Cancel).await.ok);
    server.await.unwrap();
}

#[tokio::test]
async fn second_toggle_stops_capture_and_announces_transcription_before_result() {
    let dir = tempfile::tempdir().unwrap();
    let shell_path = dir.path().join("shell.sock");
    let listener = UnixListener::bind(&shell_path).unwrap();
    let (sent, mut notifications) = tokio::sync::mpsc::channel(16);
    let shell = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let line = ipc::read_line(&mut BufReader::new(&mut stream))
                .await
                .unwrap()
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&line).unwrap();
            sent.send(value).await.unwrap();
            ipc::write_json(&mut stream, &serde_json::json!({"type":"reply","ok":true}))
                .await
                .unwrap();
        }
    });
    let (endpoint, server) = http_server(
        Duration::from_millis(700),
        r#"{"text":"private recognized words"}"#,
    )
    .await;
    let h = Harness::with_shell(endpoint, None, Some(&shell_path), |c| {
        c.output.notify = true
    })
    .await;
    assert_eq!(h.call(Action::Toggle).await.status.phase, Phase::Recording);
    let listening = tokio::time::timeout(Duration::from_secs(3), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(listening["summary"], "Listening");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let stopped = h.call(Action::Toggle).await;
    assert!(stopped.ok);
    assert_eq!(stopped.status.phase, Phase::Transcribing);
    let transcribing = tokio::time::timeout(Duration::from_secs(3), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(transcribing["summary"], "Transcribing");
    assert!(
        transcribing["body"]
            .as_str()
            .unwrap()
            .contains("Recording stopped")
    );
    let busy = h.call(Action::Toggle).await;
    assert!(!busy.ok);
    assert!(busy.error.unwrap().contains("recording has stopped"));
    let reminder = tokio::time::timeout(Duration::from_secs(3), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reminder["summary"], "Transcribing");
    assert!(
        reminder["body"]
            .as_str()
            .unwrap()
            .contains("microphone is off")
    );
    h.wait_for(Phase::Idle).await;
    let complete = tokio::time::timeout(Duration::from_secs(3), notifications.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(complete["summary"], "Dictation complete");
    for value in [listening, transcribing, reminder, complete] {
        assert!(!value.to_string().contains("private recognized words"));
    }
    server.await.unwrap();
    shell.abort();
}
