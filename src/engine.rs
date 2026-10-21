use crate::{
    audio,
    config::{Backend, Engine},
};
use anyhow::{Context, Result, bail, ensure};
#[cfg(feature = "local")]
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub struct Recognizer {
    config: Engine,
    #[cfg(feature = "local")]
    context: Arc<Mutex<Option<whisper_rs::WhisperContext>>>,
    loaded: Arc<AtomicBool>,
}
impl Recognizer {
    pub fn new(config: Engine) -> Self {
        // Native debug logs can contain decoded tokens. Keep them out of the
        // journal as well as stdout, even in development builds.
        #[cfg(feature = "local")]
        whisper_rs::install_logging_hooks();
        Self {
            config,
            #[cfg(feature = "local")]
            context: Arc::new(Mutex::new(None)),
            loaded: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn loaded(&self) -> bool {
        self.loaded.load(Ordering::Relaxed)
    }
    pub fn unload(&self) -> Result<()> {
        #[cfg(feature = "local")]
        {
            *self
                .context
                .try_lock()
                .map_err(|_| anyhow::anyhow!("model is busy"))? = None;
        }
        self.loaded.store(false, Ordering::Relaxed);
        Ok(())
    }
    pub async fn transcribe(&self, samples: Vec<f32>, cancel: Arc<AtomicBool>) -> Result<String> {
        ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
        match self.config.backend {
            Backend::Local => {
                let this = self.clone();
                tokio::task::spawn_blocking(move || this.local(&samples, cancel))
                    .await
                    .context("recognizer worker failed")?
            }
            Backend::Http => self.http(&samples, cancel).await,
        }
    }
    #[cfg(not(feature = "local"))]
    fn local(&self, _: &[f32], _: Arc<AtomicBool>) -> Result<String> {
        bail!("this build has no local backend; build with the default local feature");
    }
    #[cfg(feature = "local")]
    fn local(&self, samples: &[f32], cancel: Arc<AtomicBool>) -> Result<String> {
        use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
        ensure!(
            self.config.language == "auto"
                || whisper_rs::get_lang_id(&self.config.language).is_some(),
            "unknown Whisper language code"
        );
        let mut context = self
            .context
            .lock()
            .map_err(|_| anyhow::anyhow!("recognizer mutex poisoned"))?;
        ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
        if context.is_none() {
            ensure!(
                self.config.model.is_file(),
                "model not found: {}; run koe model download base, or set engine.model",
                self.config.model.display()
            );
            let mut params = WhisperContextParameters::default();
            params.use_gpu(self.config.gpu);
            *context = Some(
                WhisperContext::new_with_params(
                    self.config
                        .model
                        .to_str()
                        .context("model path is not UTF-8")?,
                    params,
                )
                .context("cannot load Whisper model")?,
            );
            self.loaded.store(true, Ordering::Relaxed);
        }
        ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
        let mut state = context.as_ref().unwrap().create_state()?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.config.threads as i32);
        params.set_language(if self.config.language == "auto" {
            None
        } else {
            Some(&self.config.language)
        });
        params.set_translate(self.config.translate);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_initial_prompt(&self.config.prompt);
        let abort = cancel.clone();
        params.set_abort_callback_safe(move || abort.load(Ordering::Relaxed));
        state
            .full(params, samples)
            .context("Whisper inference failed")?;
        ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
        Ok(state.as_iter().map(|s| s.to_string()).collect::<String>())
    }
    async fn http(&self, samples: &[f32], cancel: Arc<AtomicBool>) -> Result<String> {
        let c = &self.config.http;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(c.timeout_seconds))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let file = reqwest::multipart::Part::bytes(audio::wav_bytes(samples)?)
            .file_name("dictation.wav")
            .mime_str("audio/wav")?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", file)
            .text("model", c.model.clone())
            .text("response_format", "json");
        if self.config.language != "auto" {
            form = form.text("language", self.config.language.clone());
        }
        if !self.config.prompt.is_empty() {
            form = form.text("prompt", self.config.prompt.clone());
        }
        if self.config.translate {
            form = form.text("translate", "true");
        }
        let mut request = client.post(&c.endpoint).multipart(form);
        if let Some(var) = &c.api_key_env {
            let key = std::env::var(var)
                .with_context(|| format!("credential environment variable {var} is unset"))?;
            ensure!(!key.is_empty(), "credential environment variable is empty");
            request = request.bearer_auth(key);
        }
        // Do not include server response bodies or URLs in errors: they may contain speech or credentials.
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("transcription HTTP request failed"))?;
        ensure!(
            response.status().is_success(),
            "transcription server returned HTTP {}",
            response.status().as_u16()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("cannot read transcription response"))?
        {
            ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
            ensure!(
                bytes.len() + chunk.len() <= 1024 * 1024,
                "transcription response exceeds 1 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("transcription response must be JSON")?;
        match value.get("text").and_then(|t| t.as_str()) {
            Some(text) => Ok(text.to_owned()),
            None => bail!("transcription response must have a string text field"),
        }
    }
}
