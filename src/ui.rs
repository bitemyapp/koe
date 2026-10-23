//! The native Aizome vocabulary window. All recognition stays in the daemon.
//!
//! Requests that change something (`request`) disable the controls until
//! they finish. Reads (`read`: status, teaching polls, the library and saved
//! examples) leave them alone, so polling doesn't make the window flicker.
use crate::{
    ipc,
    protocol::{
        Action, Phase, Request, Response, Status, TeachingPreview, VocabularyReply,
        VocabularyRequest as V,
    },
    vocabulary::{self, ExampleSummary, Library, Pack, Word},
};
use aizome_kit::{column, prelude::*};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::Duration,
};

/// Ticks (half a second each) between reads of the library, and of the
/// selected word's examples: changes from `koe packs`, agents and other
/// windows show within three seconds.
const REREAD_TICKS: u64 = 6;

#[derive(Clone, Debug)]
pub enum Msg {
    Tick,
    Reply(V, Box<Result<Response, String>>),
    /// A read's reply, with the number of requests sent before it.
    Read(u64, Option<V>, Box<Result<Response, String>>),
    Refresh,
    Pack(String),
    Word(String),
    NewPack,
    SavePack,
    DeletePack,
    PackName(String),
    Description(String),
    Enabled(bool),
    NewWord,
    Spelling(String),
    Aliases(String),
    SaveWord,
    DeleteWord,
    Record,
    Stop,
    Discard,
    Play,
    EditTranscript(EditorAction),
    SaveExample,
    ApproveAlias(String),
    DeleteExample(String),
    LoadExamples,
    Import,
    ImportFrom(Option<Vec<PathBuf>>),
    Export,
    ExportTo(Option<PathBuf>),
    ExportExamples,
    ExportExamplesTo(Option<PathBuf>),
}

pub struct VocabularyWindow {
    socket: PathBuf,
    library: Library,
    pack: String,
    word: Option<String>,
    pack_name: String,
    description: String,
    spelling: String,
    aliases: String,
    status: Status,
    token: Option<String>,
    preview: Option<TeachingPreview>,
    transcript: EditorContent,
    examples: Vec<ExampleSummary>,
    /// The pack and word `examples` belong to.
    examples_for: Option<(String, String)>,
    /// Nothing can be edited before the daemon's library arrives.
    loaded: bool,
    busy: bool,
    queued: VecDeque<V>,
    /// Requests sent; a read that crosses one may describe the state
    /// before it, so its reply is dropped.
    sent: u64,
    reading: usize,
    ticks: u64,
    offline: bool,
    recognition: String,
    closing: bool,
    window: Option<WindowKey>,
    message: String,
    error: bool,
}

impl VocabularyWindow {
    pub fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            library: Library {
                version: 1,
                revision: 0,
                enabled: vec![],
                packs: vec![],
            },
            pack: "sekka".into(),
            word: Some("aizome".into()),
            pack_name: String::new(),
            description: String::new(),
            spelling: String::new(),
            aliases: String::new(),
            status: Status::default(),
            token: None,
            preview: None,
            transcript: EditorContent::new(""),
            examples: vec![],
            examples_for: None,
            loaded: false,
            busy: false,
            queued: VecDeque::new(),
            sent: 0,
            reading: 0,
            ticks: 0,
            offline: false,
            recognition: String::new(),
            closing: false,
            window: None,
            message: "Connecting to Koe…".into(),
            error: false,
        }
    }
    fn selected_pack(&self) -> Option<&Pack> {
        self.library.packs.iter().find(|p| p.id == self.pack)
    }
    fn selected_word(&self) -> Option<&Word> {
        self.selected_pack()?
            .words
            .iter()
            .find(|w| Some(&w.id) == self.word.as_ref())
    }
    /// Show the selected pack and word as saved. A word of `None` is a new
    /// one being written; a selection that's gone falls back to the first.
    fn load_fields(&mut self) {
        if self.selected_pack().is_none() {
            self.select_pack(self.library.packs.first().map(|p| p.id.clone()));
            return;
        }
        if let Some(pack) = self.selected_pack().cloned() {
            self.pack_name = pack.name;
            self.description = pack.description;
            if self
                .word
                .as_ref()
                .is_some_and(|id| !pack.words.iter().any(|w| &w.id == id))
            {
                self.word = pack.words.first().map(|w| w.id.clone());
            }
        }
        self.load_word();
    }
    fn select_pack(&mut self, id: Option<String>) {
        self.pack = id.unwrap_or_default();
        self.word = self
            .selected_pack()
            .and_then(|p| p.words.first())
            .map(|w| w.id.clone());
        if let Some(pack) = self.selected_pack().cloned() {
            self.pack_name = pack.name;
            self.description = pack.description;
        }
        self.load_word();
    }
    fn load_word(&mut self) {
        let word = self.selected_word().cloned();
        self.spelling = word
            .as_ref()
            .map(|w| w.spelling.clone())
            .unwrap_or_default();
        self.aliases = word.map(|w| w.aliases.join("; ")).unwrap_or_default();
    }
    fn pack_edited(&self) -> bool {
        self.selected_pack()
            .is_some_and(|p| p.name != self.pack_name || p.description != self.description)
    }
    fn word_edited(&self) -> bool {
        match self.selected_word() {
            Some(w) => w.spelling != self.spelling || w.aliases.join("; ") != self.aliases,
            None => !self.spelling.is_empty() || !self.aliases.is_empty(),
        }
    }
    /// Take a newer library, keeping what the person is typing in fields
    /// whose pack or word still exists.
    fn adopt(&mut self, library: Library) {
        let (pack, word) = (self.pack.clone(), self.word.clone());
        let pack_fields = self
            .pack_edited()
            .then(|| (self.pack_name.clone(), self.description.clone()));
        let word_fields = self
            .word_edited()
            .then(|| (self.spelling.clone(), self.aliases.clone()));
        self.library = library;
        self.loaded = true;
        self.load_fields();
        if self.pack == pack {
            if let Some((name, description)) = pack_fields {
                (self.pack_name, self.description) = (name, description);
            }
            if self.word == word
                && let Some((spelling, aliases)) = word_fields
            {
                (self.spelling, self.aliases) = (spelling, aliases);
            }
        }
    }
    /// The selected word's saved examples, once they've arrived.
    fn examples(&self) -> &[ExampleSummary] {
        match (&self.examples_for, &self.word) {
            (Some((pack, word)), Some(selected)) if *pack == self.pack && word == selected => {
                &self.examples
            }
            _ => &[],
        }
    }
    fn take_examples(&mut self, command: Option<&V>, examples: Vec<ExampleSummary>) {
        // A reply for a word no longer selected is dropped.
        if let Some(V::Examples { pack, word }) = command
            && *pack == self.pack
            && Some(word) == self.word.as_ref()
        {
            self.examples = examples;
            self.examples_for = Some((pack.clone(), word.clone()));
        }
    }
    fn take_preview(&mut self, preview: TeachingPreview) {
        if self.preview.is_none() {
            self.transcript = EditorContent::new(&preview.corrected);
            self.message =
                "Review the complete transcript, then save or discard this example.".into();
            self.error = false;
        }
        self.preview = Some(preview);
    }
    fn take_status(&mut self, status: Status) {
        self.status = status;
        if self.token.is_some() && self.status.phase == Phase::Error {
            self.show_error(
                self.status
                    .error
                    .clone()
                    .unwrap_or_else(|| "The example could not be recognized.".into()),
            );
        }
    }
    fn take_reply(&mut self, command: &V, reply: VocabularyReply, cx: &mut Cx<Msg>) {
        if let Some(recognition) = reply.recognition {
            self.recognition = recognition;
        }
        if let Some(library) = reply.library {
            if let V::Import { .. } = command {
                self.library = library;
                self.loaded = true;
                self.select_pack(self.library.packs.last().map(|p| p.id.clone()));
                self.message = "Pack imported. Review its words, then enable it when ready.".into();
            } else {
                self.adopt(library);
                self.message =
                    "Active packs apply across the desktop, starting with your next dictation."
                        .into();
            }
            self.error = false;
            if self.examples_for != self.word.clone().map(|w| (self.pack.clone(), w)) {
                self.load_examples(cx);
            }
        }
        if let Some(examples) = reply.examples {
            self.take_examples(Some(command), examples);
        }
        if let Some(preview) = reply.preview {
            self.take_preview(preview);
        }
        if let Some(message) = reply.message {
            self.message = message;
            self.error = false;
        }
    }
    fn take_read(&mut self, command: Option<&V>, reply: VocabularyReply, cx: &mut Cx<Msg>) {
        if let Some(recognition) = reply.recognition {
            self.recognition = recognition;
        }
        if let Some(library) = reply.library
            && (!self.loaded || library.revision != self.library.revision)
        {
            if self.loaded {
                self.message = "Vocabulary changed outside this window; showing the latest.".into();
            } else {
                self.message =
                    "Active packs apply across the desktop, starting with your next dictation."
                        .into();
            }
            self.error = false;
            self.adopt(library);
            if self.examples_for != self.word.clone().map(|w| (self.pack.clone(), w)) {
                self.load_examples(cx);
            }
        }
        if let Some(examples) = reply.examples {
            self.take_examples(command, examples);
        }
        if let Some(preview) = reply.preview {
            self.take_preview(preview);
        }
    }
    /// Send a request that changes something; the controls wait for it.
    fn request(&mut self, command: V, cx: &mut Cx<Msg>) {
        if self.busy {
            self.queued.push_back(command);
            return;
        }
        self.busy = true;
        self.sent += 1;
        let socket = self.socket.clone();
        cx.spawn(move || {
            let result = exchange(&socket, Request::vocabulary(command.clone()));
            Msg::Reply(command, Box::new(result))
        });
    }
    /// Read status (`None`) or vocabulary without disabling anything.
    fn read(&mut self, command: Option<V>, cx: &mut Cx<Msg>) {
        self.reading += 1;
        let (socket, sent) = (self.socket.clone(), self.sent);
        cx.spawn(move || {
            let request = command
                .clone()
                .map(Request::vocabulary)
                .unwrap_or_else(|| Request::new(Action::Status));
            Msg::Read(sent, command, Box::new(exchange(&socket, request)))
        });
    }
    fn save_library(&mut self, library: Library, cx: &mut Cx<Msg>) {
        self.request(V::SaveLibrary { library }, cx);
    }
    fn show_error(&mut self, error: impl Into<String>) {
        self.message = error.into();
        self.error = true;
    }
    fn finish_close(&mut self, cx: &mut Cx<Msg>) {
        if self.busy {
            return;
        }
        if let Some(token) = self.token.clone() {
            self.request(V::TeachCancel { token }, cx);
        } else if let Some(window) = self.window {
            cx.close_window(window);
        }
    }
    fn aliases(&self) -> Vec<String> {
        self.aliases
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    }
    fn word_is_saved(&self) -> bool {
        self.selected_word()
            .is_some_and(|w| w.spelling == self.spelling.trim() && w.aliases == self.aliases())
    }
    fn examples_request(&self) -> Option<V> {
        self.word.clone().map(|word| V::Examples {
            pack: self.pack.clone(),
            word,
        })
    }
    fn load_examples(&mut self, cx: &mut Cx<Msg>) {
        if let Some(command) = self.examples_request() {
            self.read(Some(command), cx);
        }
    }
    fn panel(&self, title: &str) -> FileDialog {
        FileDialog::new().title(title).directory(
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
        )
    }
}

impl Program for VocabularyWindow {
    type Msg = Msg;
    fn info(&self) -> AppInfo {
        AppInfo::new(
            "org.aizome.Koe.Vocabulary",
            "Koe",
            "Vocabulary and teaching",
        )
        .icon(Icon::from_name("mic").unwrap_or(Icon::Settings))
    }
    fn launched(&mut self, cx: &mut Cx<Msg>) {
        self.window = Some(
            cx.open_window(
                WindowSpec::new("Koe — Vocabulary and teaching")
                    .size(960.0, 740.0)
                    .min_size(760.0, 540.0),
            ),
        );
        cx.every(Duration::from_millis(500), Msg::Tick);
        self.read(Some(V::Library), cx);
    }
    fn close_requested(&mut self, _window: WindowKey, cx: &mut Cx<Msg>) -> bool {
        self.closing = true;
        self.queued.clear();
        if !self.busy && self.token.is_none() {
            return true;
        }
        self.message = "Closing teaching session…".into();
        self.finish_close(cx);
        false
    }
    fn update(&mut self, msg: Msg, cx: &mut Cx<Msg>) {
        match msg {
            Msg::Tick => {
                self.ticks += 1;
                if !self.busy && self.reading == 0 && !self.closing {
                    let command = match self.token.clone() {
                        Some(token) => Some(V::TeachPoll { token }),
                        None if !self.loaded || self.ticks.is_multiple_of(REREAD_TICKS) => {
                            Some(V::Library)
                        }
                        None if self.ticks % REREAD_TICKS == REREAD_TICKS / 2 => {
                            self.examples_request()
                        }
                        None => None,
                    };
                    self.read(command, cx);
                }
            }
            Msg::Read(sent, command, result) => {
                self.reading -= 1;
                // A request sent since may have changed what this describes.
                if sent != self.sent || self.closing {
                    return;
                }
                match *result {
                    Err(error) => {
                        self.offline = true;
                        self.show_error(error);
                    }
                    Ok(response) => {
                        if std::mem::take(&mut self.offline) {
                            self.message = "Connected to Koe.".into();
                            self.error = false;
                        }
                        self.take_status(response.status);
                        if !response.ok {
                            if matches!(command, Some(V::TeachPoll { .. })) {
                                self.token = None;
                                self.preview = None;
                            }
                            self.show_error(
                                response
                                    .error
                                    .unwrap_or_else(|| "Koe could not complete the request".into()),
                            );
                        } else if let Some(reply) = response.vocabulary {
                            self.take_read(command.as_ref(), reply, cx);
                        }
                    }
                }
            }
            Msg::Reply(command, result) => {
                self.busy = false;
                match *result {
                    Err(error) => {
                        self.show_error(error);
                        // On close, the daemon's lease also covers transport failure.
                        if self.closing {
                            self.token = None;
                        }
                    }
                    Ok(response) => {
                        self.take_status(response.status);
                        if !response.ok {
                            let error = response
                                .error
                                .unwrap_or_else(|| "Koe could not complete the request".into());
                            if matches!(command, V::TeachStart { .. } | V::TeachCancel { .. })
                                || !self.status.teaching
                            {
                                self.token = None;
                                self.preview = None;
                            }
                            self.show_error(error);
                            if self.selected_pack().is_none() {
                                self.load_fields();
                            }
                        } else {
                            if matches!(command, V::TeachStop { .. }) {
                                self.message = "Microphone off. Recognizing your example…".into();
                                self.error = false;
                            }
                            if matches!(command, V::TeachCancel { .. }) {
                                self.message =
                                    "Example discarded. The recording was not saved.".into();
                                self.error = false;
                            }
                            if matches!(command, V::TeachCancel { .. } | V::TeachSave { .. }) {
                                self.token = None;
                                self.preview = None;
                                self.transcript = EditorContent::new("");
                            }
                            if matches!(
                                command,
                                V::TeachCancel { .. }
                                    | V::TeachSave { .. }
                                    | V::DeleteExample { .. }
                            ) {
                                cx.send(Msg::LoadExamples);
                            }
                            if let Some(reply) = response.vocabulary {
                                self.take_reply(&command, reply, cx);
                            }
                        }
                    }
                }
                if self.closing {
                    self.finish_close(cx);
                } else if let Some(command) = self.queued.pop_front() {
                    self.request(command, cx);
                }
            }
            Msg::Refresh if self.token.is_none() => self.request(V::Library, cx),
            Msg::Pack(id) if self.token.is_none() && !self.busy => {
                self.select_pack(Some(id));
                self.load_examples(cx);
            }
            Msg::Word(id) if self.token.is_none() && !self.busy => {
                self.word = Some(id);
                self.load_word();
                self.load_examples(cx);
            }
            Msg::PackName(s) => self.pack_name = s,
            Msg::Description(s) => self.description = s,
            Msg::Spelling(s) => self.spelling = s,
            Msg::Aliases(s) => self.aliases = s,
            Msg::Enabled(on) => {
                let mut library = self.library.clone();
                library.enabled.retain(|p| p != &self.pack);
                if on {
                    library.enabled.push(self.pack.clone());
                }
                self.save_library(library, cx);
            }
            Msg::NewPack => match vocabulary::new_id() {
                Ok(id) => {
                    let mut library = self.library.clone();
                    library.packs.push(Pack {
                        id: id.clone(),
                        name: "New vocabulary pack".into(),
                        description: String::new(),
                        words: vec![],
                    });
                    self.pack = id;
                    self.word = None;
                    self.spelling.clear();
                    self.aliases.clear();
                    self.save_library(library, cx);
                }
                Err(error) => self.show_error(error.to_string()),
            },
            Msg::SavePack => {
                let mut library = self.library.clone();
                if let Some(pack) = library.packs.iter_mut().find(|p| p.id == self.pack) {
                    pack.name = self.pack_name.trim().into();
                    pack.description = self.description.trim().into();
                    self.save_library(library, cx);
                }
            }
            Msg::DeletePack => {
                let mut library = self.library.clone();
                library.packs.retain(|p| p.id != self.pack);
                library.enabled.retain(|id| id != &self.pack);
                self.save_library(library, cx);
            }
            Msg::NewWord => {
                self.word = None;
                self.spelling.clear();
                self.aliases.clear();
                cx.focus("spelling");
            }
            Msg::SaveWord => {
                let mut library = self.library.clone();
                let id = self.word.clone().map(Ok).unwrap_or_else(vocabulary::new_id);
                match id {
                    Ok(id) => {
                        if let Some(pack) = library.packs.iter_mut().find(|p| p.id == self.pack) {
                            pack.words.retain(|w| w.id != id);
                            pack.words.push(Word {
                                id: id.clone(),
                                spelling: self.spelling.trim().into(),
                                aliases: self.aliases(),
                            });
                            self.word = Some(id);
                            self.save_library(library, cx);
                        }
                    }
                    Err(error) => self.show_error(error.to_string()),
                }
            }
            Msg::DeleteWord => {
                let mut library = self.library.clone();
                if let Some(pack) = library.packs.iter_mut().find(|p| p.id == self.pack) {
                    pack.words.retain(|w| Some(&w.id) != self.word.as_ref());
                    self.save_library(library, cx);
                }
            }
            Msg::Record if self.token.is_none() && !self.busy && self.word_is_saved() => {
                if let Some(word) = self.word.clone() {
                    match vocabulary::new_id() {
                        Ok(token) => {
                            self.token = Some(token.clone());
                            self.preview = None;
                            self.error = false;
                            self.message =
                                "Recording an example. Stop when you finish speaking.".into();
                            self.request(
                                V::TeachStart {
                                    token,
                                    pack: self.pack.clone(),
                                    word,
                                },
                                cx,
                            );
                        }
                        Err(error) => self.show_error(error.to_string()),
                    }
                }
            }
            Msg::Stop => {
                if let Some(token) = self.token.clone() {
                    self.request(V::TeachStop { token }, cx);
                }
            }
            Msg::Discard => {
                if let Some(token) = self.token.clone() {
                    self.request(V::TeachCancel { token }, cx);
                }
            }
            Msg::Play => {
                if let Some(token) = self.token.clone() {
                    self.request(V::TeachPlay { token }, cx);
                }
            }
            Msg::SaveExample => {
                if let Some(token) = self.token.clone() {
                    self.request(
                        V::TeachSave {
                            token,
                            transcript: self.transcript.text(),
                        },
                        cx,
                    );
                }
            }
            Msg::EditTranscript(action) => {
                self.transcript.perform(action);
            }
            Msg::ApproveAlias(alias) => {
                let mut library = self.library.clone();
                if let Some(word) = library
                    .packs
                    .iter_mut()
                    .find(|p| p.id == self.pack)
                    .and_then(|p| {
                        p.words
                            .iter_mut()
                            .find(|w| Some(&w.id) == self.word.as_ref())
                    })
                {
                    word.aliases.push(alias);
                    self.save_library(library, cx);
                }
            }
            Msg::DeleteExample(id) => self.request(V::DeleteExample { id }, cx),
            Msg::LoadExamples if !self.closing => self.load_examples(cx),
            Msg::Import => {
                if let Some(window) = self.window {
                    cx.open_panel(
                        window,
                        self.panel("Import vocabulary pack")
                            .filter("Koe vocabulary", &["json"]),
                        Msg::ImportFrom,
                    );
                }
            }
            Msg::ImportFrom(Some(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    self.request(
                        V::Import {
                            path,
                            revision: self.library.revision,
                        },
                        cx,
                    );
                }
            }
            Msg::Export => {
                if let Some(window) = self.window {
                    cx.save_panel(
                        window,
                        self.panel("Export vocabulary only")
                            .file_name(format!("{}.koe.json", self.pack)),
                        Msg::ExportTo,
                    );
                }
            }
            Msg::ExportTo(Some(path)) => self.request(
                V::Export {
                    pack: self.pack.clone(),
                    path,
                },
                cx,
            ),
            Msg::ExportExamples => {
                if let Some(window) = self.window {
                    cx.save_panel(
                        window,
                        self.panel("Export recordings and transcripts to a new folder")
                            .file_name(format!("{}-training", self.pack)),
                        Msg::ExportExamplesTo,
                    );
                }
            }
            Msg::ExportExamplesTo(Some(path)) => self.request(
                V::ExportExamples {
                    pack: self.pack.clone(),
                    path,
                },
                cx,
            ),
            _ => (),
        }
    }
    fn view(&self, _window: WindowKey) -> Element<Msg> {
        let editable = self.loaded && !self.busy && self.token.is_none();
        let mut packs: Vec<Element<Msg>> = vec![];
        for pack in &self.library.packs {
            packs.push(
                column![
                    button(pack.name.clone())
                        .selected(pack.id == self.pack)
                        .on_press(Msg::Pack(pack.id.clone()))
                        .enabled(editable)
                        .width(Fill)
                        .id(format!("pack:{}", pack.id)),
                    text(format!(
                        "{} · {} words",
                        if self.library.enabled.contains(&pack.id) {
                            "Active"
                        } else {
                            "Off"
                        },
                        pack.words.len()
                    ))
                    .role(Role::Text3)
                    .size(11.0),
                ]
                .spacing(3.0)
                .key(pack.id.clone())
                .into(),
            );
        }
        let sidebar = container(
            column![
                text("Vocabulary packs").semibold(),
                text("Choose the words Koe should expect.")
                    .role(Role::Text2)
                    .wrap(),
                row![
                    button("New pack").on_press(Msg::NewPack).enabled(editable),
                    button("Import…").on_press(Msg::Import).enabled(editable)
                ]
                .spacing(6.0),
                divider(),
                scroll(column(packs).spacing(10.0)).height(Fill),
                button("Refresh").on_press(Msg::Refresh).enabled(editable),
            ]
            .spacing(10.0)
            .height(Fill),
        )
        .padding(14.0)
        .fill(Tone::Chrome)
        .width(210.0)
        .height(Fill);
        let body: Element<Msg> = if let Some(pack) = self.selected_pack() {
            let mut words: Vec<Element<Msg>> = vec![
                button("Add word")
                    .primary()
                    .on_press(Msg::NewWord)
                    .enabled(editable)
                    .width(Fill)
                    .into(),
            ];
            for word in &pack.words {
                words.push(
                    button(word.spelling.clone())
                        .selected(Some(&word.id) == self.word.as_ref())
                        .on_press(Msg::Word(word.id.clone()))
                        .enabled(editable)
                        .width(Fill)
                        .id(format!("word:{}", word.id))
                        .key(word.id.clone())
                        .into(),
                );
            }
            column![
                row![
                    text("Vocabulary and teaching").size(22.0).semibold(),
                    spacer(),
                    button("Export pack…")
                        .on_press(Msg::Export)
                        .enabled(editable)
                ]
                .spacing(12.0),
                row![
                    text_field(&self.pack_name)
                        .on_change(Msg::PackName)
                        .enabled(editable)
                        .id("pack-name")
                        .label("Pack name"),
                    button("Save pack")
                        .on_press(Msg::SavePack)
                        .enabled(editable),
                    button("Remove pack")
                        .on_press(Msg::DeletePack)
                        .enabled(editable && self.library.packs.len() > 1)
                ],
                text_field(&self.description)
                    .placeholder("What this vocabulary is for")
                    .on_change(Msg::Description)
                    .enabled(editable)
                    .id("pack-description")
                    .label("Pack description"),
                checkbox(
                    "Use this pack across the desktop",
                    self.library.enabled.contains(&self.pack)
                )
                .on_toggle(Msg::Enabled)
                .enabled(editable)
                .id("pack-enabled"),
                divider(),
                row![
                    scroll(column(words).spacing(6.0)).width(150.0).height(Fill),
                    divider().vertical(),
                    scroll(self.editor()).width(Fill).height(Fill)
                ]
                .spacing(14.0)
                .height(Fill),
            ]
            .padding(18.0)
            .spacing(12.0)
            .width(Fill)
            .height(Fill)
            .into()
        } else {
            container(text(if self.loaded {
                "Create or import a vocabulary pack to begin."
            } else {
                "Loading vocabulary…"
            }))
            .padding(24.0)
            .width(Fill)
            .height(Fill)
            .into()
        };
        column![
            row![sidebar, body].height(Fill),
            container(
                text(self.message.as_str())
                    .role(if self.error {
                        Role::Danger
                    } else {
                        Role::Text2
                    })
                    .wrap()
                    .max_lines(3)
            )
            .padding([8.0, 14.0])
            .fill(Tone::Chrome)
            .width(Fill),
        ]
        .height(Fill)
        .into()
    }
}

impl VocabularyWindow {
    fn editor(&self) -> Element<Msg> {
        let idle = !self.busy && self.token.is_none();
        let can_edit_aliases = !self.busy && (self.token.is_none() || self.preview.is_some());
        let mut content: Vec<Element<Msg>> = if self.token.is_none() {
            vec![
            text("Written form").semibold().into(),
            text_field(&self.spelling).id("spelling").label("Written form").placeholder("For example, Aizome").on_change(Msg::Spelling).enabled(idle).into(),
            text("Approved corrections").semibold().into(),
            text_field(&self.aliases).id("aliases").label("Approved corrections").placeholder("Isome; IZOMA").on_change(Msg::Aliases).enabled(can_edit_aliases).into(),
            text("Separate variants with semicolons. Corrections match whole words and ignore case.").role(Role::Text2).wrap().into(),
            row![button("Save word").primary().on_press(Msg::SaveWord).enabled(can_edit_aliases && !self.spelling.trim().is_empty() && !self.word_is_saved()),
                button("Remove word").on_press(Msg::DeleteWord).enabled(idle && self.word.is_some() && self.examples().is_empty())].spacing(8.0).into(),
            divider().into(),
            text("Teach by example").size(18.0).semibold().into(),
            text("Say this word in a short sentence. Review what you actually said before saving a recording for future training.")
                .role(Role::Text2).wrap().into(),
        ]
        } else {
            vec![
                text(format!("Teaching: {}", self.spelling))
                    .size(18.0)
                    .semibold()
                    .into(),
            ]
        };
        content.push(
            text(self.recognition.as_str())
                .role(Role::Text3)
                .wrap()
                .into(),
        );
        if let Some(preview) = &self.preview {
            content.extend([
                text(format!(
                    "Microphone off · {:.1} seconds",
                    preview.duration_ms as f64 / 1000.0
                ))
                .role(Role::Success)
                .into(),
                text("Recognizer heard").semibold().into(),
                text(preview.heard.as_str())
                    .wrap()
                    .max_lines(5)
                    .selectable()
                    .into(),
                text("Correct transcript — the complete sentence")
                    .semibold()
                    .into(),
                container(
                    text_editor(&self.transcript)
                        .on_action(Msg::EditTranscript)
                        .wrap(true)
                        .height(90.0)
                        .id("correct-transcript")
                        .label("Correct transcript"),
                )
                .border(Edge::Line)
                .into(),
            ]);
            if let Some(alias) =
                suggested_alias(&preview.heard, &self.transcript.text(), &self.spelling)
                    .filter(|alias| !self.aliases().iter().any(|a| a.eq_ignore_ascii_case(alias)))
            {
                content.push(
                    button(format!("Add correction: {alias} → {}", self.spelling))
                        .on_press(Msg::ApproveAlias(alias))
                        .enabled(!self.busy)
                        .into(),
                );
            }
            content.push(
                row![
                    button("Play sample")
                        .on_press(Msg::Play)
                        .enabled(!self.busy),
                    button("Save example")
                        .primary()
                        .on_press(Msg::SaveExample)
                        .enabled(!self.busy && !self.transcript.text().trim().is_empty()),
                    button("Discard").on_press(Msg::Discard).enabled(!self.busy)
                ]
                .spacing(8.0)
                .into(),
            );
            content.push(text("Save keeps this recording and your corrected transcript on this computer. No model training runs yet.").role(Role::Text2).wrap().into());
        } else if self.token.is_some() {
            let recording = matches!(self.status.phase, Phase::Recording | Phase::Paused);
            content.push(
                text(match self.status.phase {
                    Phase::Recording => "Microphone on — speak, then stop",
                    Phase::Transcribing | Phase::Finishing => {
                        "Microphone off — recognizing your example…"
                    }
                    Phase::Error => "Example failed. Discard it and try again.",
                    _ => "Starting teaching session…",
                })
                .role(if recording { Role::Danger } else { Role::Text2 })
                .wrap()
                .into(),
            );
            content.push(
                text(format!(
                    "{:.1} seconds / 30 · input level {:.0}%",
                    self.status.elapsed_ms as f64 / 1000.0,
                    (self.status.level * 500.0).min(100.0)
                ))
                .role(Role::Text2)
                .into(),
            );
            content.push(
                row![
                    button("Stop recording")
                        .primary()
                        .on_press(Msg::Stop)
                        .enabled(recording && !self.busy),
                    button("Discard").on_press(Msg::Discard).enabled(!self.busy)
                ]
                .spacing(8.0)
                .into(),
            );
        } else {
            content.push(
                button("Record example")
                    .primary()
                    .on_press(Msg::Record)
                    .enabled(
                        idle && self.word_is_saved()
                            && matches!(self.status.phase, Phase::Idle | Phase::Error)
                            && !self.status.teaching,
                    )
                    .into(),
            );
            content.push(text("Recording starts only when you press Record example. Unsaved examples are discarded when this window closes.").role(Role::Text2).wrap().into());
        }
        content.push(divider().into());
        content.push(
            row![
                text(format!("Saved examples ({})", self.examples().len())).semibold(),
                spacer(),
                button("Export training data…")
                    .on_press(Msg::ExportExamples)
                    .enabled(idle)
            ]
            .spacing(8.0)
            .into(),
        );
        if self.examples().is_empty() {
            content.push(
                text("No saved examples for this word.")
                    .role(Role::Text3)
                    .into(),
            );
        }
        for example in self.examples() {
            content.push(
                row![
                    column![
                        text(example.transcript.as_str()).wrap().max_lines(2),
                        text(format!(
                            "{:.1} seconds",
                            example.duration_ms as f64 / 1000.0
                        ))
                        .role(Role::Text3)
                    ]
                    .width(Fill),
                    button("Delete")
                        .destructive()
                        .on_press(Msg::DeleteExample(example.id.clone()))
                        .enabled(idle)
                        .id(format!("delete-example:{}", example.id))
                        .label(format!("Delete example: {}", example.transcript))
                ]
                .spacing(8.0)
                .key(example.id.clone())
                .into(),
            );
        }
        column(content).spacing(10.0).width(Fill).into()
    }
}

/// Offer only one isolated correction with otherwise matching context. The
/// user must approve it; a full sentence is never turned into an alias.
pub fn suggested_alias(heard: &str, corrected: &str, spelling: &str) -> Option<String> {
    let heard: Vec<_> = heard.split_whitespace().collect();
    let corrected: Vec<_> = corrected.split_whitespace().collect();
    let prefix = heard
        .iter()
        .zip(&corrected)
        .take_while(|(a, b)| a.to_lowercase() == b.to_lowercase())
        .count();
    let suffix = heard[prefix..]
        .iter()
        .rev()
        .zip(corrected[prefix..].iter().rev())
        .take_while(|(a, b)| a.to_lowercase() == b.to_lowercase())
        .count();
    let changed = corrected[prefix..corrected.len() - suffix].join(" ");
    let trim = |s: &str| {
        s.trim_matches(|c: char| ".,!?\"'“”‘’".contains(c))
            .to_string()
    };
    if trim(&changed).to_lowercase() != spelling.trim().to_lowercase() {
        return None;
    }
    let alias = trim(&heard[prefix..heard.len() - suffix].join(" "));
    (!alias.is_empty() && alias.len() <= 128 && alias.to_lowercase() != spelling.to_lowercase())
        .then_some(alias)
}

fn exchange(socket: &Path, request: Request) -> Result<Response, String> {
    (|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(ipc::request(socket, &request))
    })()
    .map_err(|e: anyhow::Error| format!("{e:#}"))
}

pub fn run(socket: PathBuf) -> anyhow::Result<()> {
    aizome_kit::try_run(VocabularyWindow::new(socket))
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("cannot open vocabulary window: {e}"))
}

#[cfg(test)]
mod tests {
    use super::suggested_alias;
    #[test]
    fn suggests_only_an_isolated_changed_word_or_phrase() {
        assert_eq!(
            suggested_alias("Open Isome settings.", "Open Aizome settings.", "Aizome").as_deref(),
            Some("Isome")
        );
        assert_eq!(
            suggested_alias("I use eye zome.", "I use Aizome.", "Aizome").as_deref(),
            Some("eye zome")
        );
        for (heard, corrected) in [
            ("", ""),
            ("Aizome", "Aizome"),
            ("Open Isome", "I use Aizome"),
            ("Isome Isome", "Aizome Aizome"),
            ("Use", "Use Aizome"),
        ] {
            assert_eq!(suggested_alias(heard, corrected, "Aizome"), None);
        }
    }
}
