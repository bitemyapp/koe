use crate::{
    audio::{self, Capture, Meter},
    config::{Config, OutputMode},
    desktop,
    engine::Recognizer,
    ipc::{self, SocketGuard},
    protocol::{Action, Phase, Request, Response, Status, VERSION},
    text::Processor,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    future::pending,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::BufReader,
    net::UnixStream,
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
};

type Command = (Request, oneshot::Sender<Response>, oneshot::Receiver<()>);
struct State {
    config: Config,
    status: Status,
    updates: watch::Sender<Status>,
    recognizer: Recognizer,
    processor: Processor,
    capture: Option<Capture>,
    meter_rx: mpsc::Receiver<Meter>,
    samples: Vec<f32>,
    inference: Option<JoinHandle<Result<String>>>,
    delivery: Option<JoinHandle<Result<()>>>,
    cancelled: Arc<AtomicBool>,
    last: Option<String>,
    target: Option<u64>,
    reported_phase: Phase,
}
impl State {
    fn publish(&mut self) {
        if self.status.phase != self.reported_phase {
            // State transitions only: no audio, recognized text or 10 Hz meter logs.
            let phase = serde_json::to_value(self.status.phase).expect("phase serializes");
            eprintln!("Koe: {}", phase.as_str().expect("phase is a string"));
            self.reported_phase = self.status.phase;
        }
        self.status.sequence += 1;
        self.status.has_last = self.last.is_some();
        self.status.model_loaded = self.recognizer.loaded();
        self.updates.send_replace(self.status.clone());
    }
    fn fail(&mut self, error: anyhow::Error) {
        self.status.phase = Phase::Error;
        self.status.level = 0.0;
        let message = format!("{error:#}");
        eprintln!("Koe: {message}");
        self.status.error = Some(message.clone());
        self.samples.clear();
        if self.config.output.notify {
            desktop::notify("Dictation needs attention", message);
        }
        self.publish();
    }
    fn begin_capture(&mut self) -> Result<()> {
        let (tx, rx) = mpsc::channel(4);
        let remaining = self.config.audio.max_seconds as usize * audio::RATE - self.samples.len();
        ensure!(
            remaining > 0,
            "recording has reached audio.max_seconds; stop to transcribe"
        );
        self.capture = Some(Capture::start(self.config.audio.clone(), remaining, tx)?);
        self.meter_rx = rx;
        self.status.phase = Phase::Recording;
        self.status.error = None;
        Ok(())
    }
    async fn stop_capture(&mut self) -> Result<()> {
        if let Some(capture) = self.capture.take() {
            self.samples.extend(capture.finish().await?);
            self.status.elapsed_ms = (self.samples.len() * 1000 / audio::RATE) as u64;
        }
        self.status.level = 0.0;
        Ok(())
    }
    fn recognize(&mut self) -> Result<()> {
        ensure!(
            self.samples.len() * 1000 / audio::RATE >= self.config.audio.min_ms as usize,
            "recording is too short"
        );
        ensure!(
            audio::rms(&self.samples) >= self.config.audio.silence_threshold,
            "no speech detected; check microphone level"
        );
        let samples = std::mem::take(&mut self.samples);
        let recognizer = self.recognizer.clone();
        self.cancelled = Arc::new(AtomicBool::new(false));
        let cancel = self.cancelled.clone();
        self.inference = Some(tokio::spawn(async move {
            recognizer.transcribe(samples, cancel).await
        }));
        self.status.phase = Phase::Transcribing;
        if self.config.output.notify {
            desktop::notify(
                "Transcribing",
                "Recording stopped. Turning your speech into text…",
            );
        }
        Ok(())
    }
    async fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Some(task) = self.inference.take() {
            task.abort();
        }
        if let Some(task) = self.delivery.take() {
            task.abort();
        }
        let _ = self.stop_capture().await;
        self.samples.clear();
        self.status.phase = Phase::Idle;
        self.status.error = None;
        self.status.elapsed_ms = 0;
        self.status.level = 0.0;
    }
    async fn command(&mut self, action: Action) -> Result<Option<String>> {
        let action = if action == Action::Toggle {
            match self.status.phase {
                Phase::Recording | Phase::Paused => Action::Stop,
                Phase::Idle | Phase::Error => Action::Start,
                Phase::Transcribing => {
                    if self.config.output.notify {
                        desktop::notify(
                            "Transcribing",
                            "Your microphone is off. Wait for the result, or cancel dictation.",
                        );
                    }
                    bail!("recording has stopped; transcription is still running")
                }
                _ => bail!("dictation is busy; wait or cancel"),
            }
        } else {
            action
        };
        match action {
            Action::Start => {
                ensure!(
                    matches!(self.status.phase, Phase::Idle | Phase::Error),
                    "dictation is already active"
                );
                self.samples.clear();
                self.status.elapsed_ms = 0;
                self.target = if desktop::uses_aizome(&self.config.output)
                    && self.config.output.mode == OutputMode::Paste
                {
                    let target = desktop::focused_window().await?;
                    ensure!(
                        !self.config.output.require_same_window || target.is_some(),
                        "focus the window to dictate into first"
                    );
                    target
                } else {
                    None
                };
                if self.config.engine.backend == crate::config::Backend::Local {
                    ensure!(
                        self.config.engine.model.is_file(),
                        "model is missing; run koe model download base or set engine.model"
                    );
                }
                self.begin_capture()?;
                if self.config.output.notify {
                    desktop::notify(
                        "Listening",
                        "Speak, then press the dictation shortcut again to finish.",
                    );
                }
            }
            Action::Stop => {
                ensure!(
                    matches!(self.status.phase, Phase::Recording | Phase::Paused),
                    "no recording to stop"
                );
                self.status.phase = Phase::Finishing;
                self.publish();
                if let Err(e) = self.stop_capture().await.and_then(|_| self.recognize()) {
                    let message = format!("{e:#}");
                    self.fail(e);
                    bail!("{message}");
                }
            }
            Action::Pause => {
                ensure!(self.status.phase == Phase::Recording, "not recording");
                if let Err(e) = self.stop_capture().await {
                    let message = format!("{e:#}");
                    self.fail(e);
                    bail!("{message}");
                } else {
                    self.status.phase = Phase::Paused;
                }
            }
            Action::Resume => {
                ensure!(self.status.phase == Phase::Paused, "not paused");
                self.begin_capture()?;
            }
            Action::Cancel | Action::Shutdown => self.cancel().await,
            Action::Status | Action::Subscribe => (),
            Action::Last => return Ok(Some(self.last.clone().context("no transcript in memory")?)),
            Action::CopyLast => {
                desktop::copy(self.last.as_deref().context("no transcript in memory")?).await?
            }
            Action::PasteLast => {
                let text = self.last.as_deref().context("no transcript in memory")?;
                let mut output = self.config.output.clone();
                output.mode = OutputMode::Paste;
                let target = if desktop::uses_aizome(&output) {
                    desktop::focused_window().await?
                } else {
                    None
                };
                desktop::deliver(text, &output, target).await?;
            }
            Action::ClearLast => self.last = None,
            Action::Unload => {
                ensure!(
                    matches!(self.status.phase, Phase::Idle | Phase::Error),
                    "cancel or finish dictation before unloading"
                );
                self.recognizer.unload()?;
            }
            Action::Toggle => unreachable!(),
        }
        Ok(None)
    }
}
async fn serve(
    stream: UnixStream,
    tx: mpsc::Sender<Command>,
    mut updates: watch::Receiver<Status>,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let line = tokio::time::timeout(Duration::from_secs(3), ipc::read_line(&mut reader))
        .await??
        .context("empty request")?;
    let request: Request = serde_json::from_slice(&line)?;
    if request.version != VERSION {
        let mut response = Response::status(updates.borrow().clone());
        response.ok = false;
        response.error = Some(format!("unsupported protocol version; expected {VERSION}"));
        return ipc::write_json(&mut writer, &response).await;
    }
    let subscribe = request.action == Action::Subscribe;
    let (reply, response) = oneshot::channel();
    let (flushed, flush) = oneshot::channel();
    tx.send((request, reply, flush)).await?;
    ipc::write_json(&mut writer, &response.await?).await?;
    let _ = flushed.send(());
    if subscribe {
        loop {
            tokio::select! {
                result = updates.changed() => {
                    result?;
                    let status = updates.borrow_and_update().clone();
                    ipc::write_json(&mut writer, &Response::status(status)).await?;
                }
                _ = ipc::read_line(&mut reader) => break,
            }
        }
    }
    Ok(())
}

pub async fn run(config: Config, path: &Path) -> Result<()> {
    let (_guard, listener) = SocketGuard::bind(path)?;
    let (updates, rx) = watch::channel(Status::default());
    let (tx, mut commands) = mpsc::channel::<Command>(32);
    let permits = Arc::new(Semaphore::new(64));
    let (_meter_tx, meter_rx) = mpsc::channel(4);
    let mut state = State {
        recognizer: Recognizer::new(config.engine.clone()),
        processor: Processor::new(&config.replacements, config.output.append_space)?,
        config,
        status: Status::default(),
        updates,
        capture: None,
        meter_rx,
        samples: vec![],
        inference: None,
        delivery: None,
        cancelled: Arc::new(AtomicBool::new(false)),
        last: None,
        target: None,
        reported_phase: Phase::Idle,
    };
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    eprintln!("Koe listening on {}", path.display());
    loop {
        tokio::select! {
            _ = &mut interrupt => break,
            _ = terminate.recv() => break,
            connection = listener.accept() => {
                let (stream, _) = connection?;
                if let Ok(permit) = permits.clone().try_acquire_owned() {
                    let tx = tx.clone(); let rx = rx.clone();
                    tokio::spawn(async move { let _permit = permit; let _ = serve(stream, tx, rx).await; });
                }
            }
            Some((request, reply, flushed)) = commands.recv() => {
                let result = state.command(request.action).await;
                // Read-only polling must not generate events or wake the shell.
                if !matches!(request.action, Action::Status | Action::Subscribe | Action::Last) { state.publish(); }
                let mut response = Response::status(state.status.clone());
                match result {
                    Ok(text) => response.text = text,
                    Err(e) => { response.ok = false; response.error = Some(format!("{e:#}")); }
                }
                let _ = reply.send(response);
                if request.action == Action::Shutdown {
                    let _ = tokio::time::timeout(Duration::from_secs(1), flushed).await;
                    break;
                }
            }
            Some(meter) = state.meter_rx.recv(), if state.capture.is_some() => {
                state.status.elapsed_ms = ((state.samples.len() + meter.samples) * 1000 / audio::RATE) as u64;
                state.status.level = meter.level;
                state.publish();
            }
            captured = async { match state.capture.as_mut() { Some(c) => (&mut c.task).await, None => pending().await } } => {
                state.capture.take();
                match captured.context("capture task failed").and_then(|v| v) {
                    Ok(samples) => {
                        state.samples.extend(samples);
                        state.status.elapsed_ms = (state.samples.len() * 1000 / audio::RATE) as u64;
                        state.status.level = 0.0;
                        if let Err(e) = state.recognize() { state.fail(e); }
                        else { state.publish(); }
                    }
                    Err(e) => state.fail(e),
                }
            }
            result = async { match state.inference.as_mut() { Some(t) => t.await, None => pending().await } } => {
                state.inference.take();
                match result.context("transcription task failed").and_then(|v| v) {
                    Ok(text) => {
                        let text = state.processor.apply(&text);
                        if text.trim().is_empty() { state.fail(anyhow::anyhow!("no speech recognized")); continue; }
                        if serde_json::to_vec(&text)?.len() > 48 * 1024 { state.fail(anyhow::anyhow!("encoded transcript exceeds 48 KiB")); continue; }
                        state.last = Some(text.clone());
                        state.status.phase = Phase::Delivering;
                        state.publish();
                        let output = state.config.output.clone(); let target = state.target;
                        state.delivery = Some(tokio::spawn(async move { desktop::deliver(&text, &output, target).await }));
                    }
                    Err(e) => state.fail(e),
                }
            }
            result = async { match state.delivery.as_mut() { Some(t) => t.await, None => pending().await } } => {
                state.delivery.take();
                match result.context("delivery task failed").and_then(|v| v) {
                    Ok(()) => {
                        state.status.phase = Phase::Idle; state.status.error = None;
                        if state.config.output.notify { desktop::notify("Dictation complete", "Your transcript is ready."); }
                        state.publish();
                    }
                    Err(e) => state.fail(e),
                }
            }
        }
    }
    state.cancel().await;
    eprintln!("Koe stopped");
    Ok(())
}
