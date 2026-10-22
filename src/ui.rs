//! The native Aizome vocabulary window. All recognition stays in the daemon.
use crate::{
    ipc,
    protocol::{Action, Phase, Request, Response, Status, TeachingPreview, VocabularyRequest as V},
    vocabulary::{self, ExampleSummary, Library, Pack, Word},
};
use aizome_kit::{column, prelude::*};
use std::{collections::VecDeque, path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
pub enum Msg {
    Tick,
    Reply(Option<V>, Box<Result<Response, String>>),
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
    busy: bool,
    queued: VecDeque<V>,
    recognition: String,
    closing: bool,
    window: Option<WindowKey>,
    message: String,
    error: bool,
}

impl VocabularyWindow {
    pub fn new(socket: PathBuf) -> Self {
        let mut app = Self {
            socket,
            library: Library::default(),
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
            busy: false,
            queued: VecDeque::new(),
            recognition: String::new(),
            closing: false,
            window: None,
            message: "Connecting to Koe…".into(),
            error: false,
        };
        app.load_fields();
        app
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
    fn load_fields(&mut self) {
        if self.selected_pack().is_none() {
            self.pack = self
                .library
                .packs
                .first()
                .map(|p| p.id.clone())
                .unwrap_or_default();
        }
        if let Some(pack) = self.selected_pack().cloned() {
            self.pack_name = pack.name;
            self.description = pack.description;
            if !pack.words.iter().any(|w| Some(&w.id) == self.word.as_ref()) {
                self.word = pack.words.first().map(|w| w.id.clone());
            }
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
        self.examples.clear();
    }
    fn request(&mut self, command: Option<V>, cx: &mut Cx<Msg>) {
        if self.busy {
            if let Some(command) = command {
                self.queued.push_back(command);
            }
            return;
        }
        self.busy = true;
        let socket = self.socket.clone();
        cx.spawn(move || {
            let result = (|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let request = command
                    .clone()
                    .map(Request::vocabulary)
                    .unwrap_or_else(|| Request::new(Action::Status));
                runtime.block_on(ipc::request(&socket, &request))
            })()
            .map_err(|e: anyhow::Error| format!("{e:#}"));
            Msg::Reply(command, Box::new(result))
        });
    }
    fn save_library(&mut self, library: Library, cx: &mut Cx<Msg>) {
        self.request(Some(V::SaveLibrary { library }), cx);
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
            self.request(Some(V::TeachCancel { token }), cx);
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
    fn example_request(&mut self, cx: &mut Cx<Msg>) {
        if let Some(word) = self.word.clone() {
            self.request(
                Some(V::Examples {
                    pack: self.pack.clone(),
                    word,
                }),
                cx,
            );
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
        self.request(Some(V::Library), cx);
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
                if !self.busy && !self.closing {
                    self.request(self.token.clone().map(|token| V::TeachPoll { token }), cx);
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
                        self.status = response.status;
                        if self.token.is_some() && self.status.phase == Phase::Error {
                            self.show_error(
                                self.status.error.clone().unwrap_or_else(|| {
                                    "The example could not be recognized.".into()
                                }),
                            );
                        }
                        if !response.ok {
                            let error = response
                                .error
                                .unwrap_or_else(|| "Koe could not complete the request".into());
                            if matches!(
                                command,
                                Some(
                                    V::TeachStart { .. }
                                        | V::TeachPoll { .. }
                                        | V::TeachCancel { .. }
                                )
                            ) || !self.status.teaching
                            {
                                self.token = None;
                                self.preview = None;
                            }
                            self.show_error(error);
                            if self.selected_pack().is_none() {
                                self.load_fields();
                            }
                        } else {
                            if matches!(command, Some(V::TeachCancel { .. } | V::TeachSave { .. }))
                            {
                                self.token = None;
                                self.preview = None;
                                self.transcript = EditorContent::new("");
                                cx.send(Msg::LoadExamples);
                            }
                            if let Some(reply) = response.vocabulary {
                                if let Some(recognition) = reply.recognition {
                                    self.recognition = recognition;
                                }
                                if let Some(library) = reply.library {
                                    if matches!(command, Some(V::Import { .. })) {
                                        self.pack = library
                                            .packs
                                            .last()
                                            .map(|p| p.id.clone())
                                            .unwrap_or_default();
                                        self.word = None;
                                    }
                                    self.library = library;
                                    self.load_fields();
                                    self.message = if matches!(command, Some(V::Import { .. })) {
                                        "Pack imported. Review its words, then enable it when ready."
                                    } else { "Active packs apply across the desktop, starting with your next dictation." }.into();
                                    self.error = false;
                                    cx.send(Msg::LoadExamples);
                                }
                                if let Some(examples) = reply.examples {
                                    self.examples = examples;
                                }
                                if let Some(preview) = reply.preview {
                                    if self.preview.is_none() {
                                        self.transcript = EditorContent::new(&preview.corrected);
                                    }
                                    self.preview = Some(preview);
                                }
                                if let Some(message) = reply.message {
                                    self.message = message;
                                    self.error = false;
                                }
                            }
                            if matches!(command, Some(V::DeleteExample { .. })) {
                                cx.send(Msg::LoadExamples);
                            }
                        }
                    }
                }
                if self.closing {
                    self.finish_close(cx);
                } else if let Some(command) = self.queued.pop_front() {
                    self.request(Some(command), cx);
                }
            }
            Msg::Refresh if self.token.is_none() => self.request(Some(V::Library), cx),
            Msg::Pack(id) if self.token.is_none() && !self.busy => {
                self.pack = id;
                self.word = None;
                self.load_fields();
                self.example_request(cx);
            }
            Msg::Word(id) if self.token.is_none() && !self.busy => {
                self.word = Some(id);
                self.load_word();
                self.example_request(cx);
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
                self.examples.clear();
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
                                Some(V::TeachStart {
                                    token,
                                    pack: self.pack.clone(),
                                    word,
                                }),
                                cx,
                            );
                        }
                        Err(error) => self.show_error(error.to_string()),
                    }
                }
            }
            Msg::Stop => {
                if let Some(token) = self.token.clone() {
                    self.request(Some(V::TeachStop { token }), cx);
                }
            }
            Msg::Discard => {
                if let Some(token) = self.token.clone() {
                    self.request(Some(V::TeachCancel { token }), cx);
                }
            }
            Msg::Play => {
                if let Some(token) = self.token.clone() {
                    self.request(Some(V::TeachPlay { token }), cx);
                }
            }
            Msg::SaveExample => {
                if let Some(token) = self.token.clone() {
                    self.request(
                        Some(V::TeachSave {
                            token,
                            transcript: self.transcript.text(),
                        }),
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
            Msg::DeleteExample(id) => self.request(Some(V::DeleteExample { id }), cx),
            Msg::LoadExamples if !self.closing => self.example_request(cx),
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
                        Some(V::Import {
                            path,
                            revision: self.library.revision,
                        }),
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
                Some(V::Export {
                    pack: self.pack.clone(),
                    path,
                }),
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
                Some(V::ExportExamples {
                    pack: self.pack.clone(),
                    path,
                }),
                cx,
            ),
            _ => (),
        }
    }
    fn view(&self, _window: WindowKey) -> Element<Msg> {
        let editable = !self.busy && self.token.is_none();
        let mut packs: Vec<Element<Msg>> = vec![];
        for pack in &self.library.packs {
            packs.push(
                column![
                    button(pack.name.clone())
                        .selected(pack.id == self.pack)
                        .on_press(Msg::Pack(pack.id.clone()))
                        .enabled(editable)
                        .width(Fill),
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
                    .enabled(editable),
                checkbox(
                    "Use this pack across the desktop",
                    self.library.enabled.contains(&self.pack)
                )
                .on_toggle(Msg::Enabled)
                .enabled(editable),
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
            container(text("Create or import a vocabulary pack to begin."))
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
            text_field(&self.spelling).id("spelling").placeholder("For example, Aizome").on_change(Msg::Spelling).enabled(idle).into(),
            text("Approved corrections").semibold().into(),
            text_field(&self.aliases).placeholder("Isome; IZOMA").on_change(Msg::Aliases).enabled(can_edit_aliases).into(),
            text("Separate variants with semicolons. Corrections match whole words and ignore case.").role(Role::Text2).wrap().into(),
            row![button("Save word").primary().on_press(Msg::SaveWord).enabled(can_edit_aliases && !self.spelling.trim().is_empty() && !self.word_is_saved()),
                button("Remove word").on_press(Msg::DeleteWord).enabled(idle && self.word.is_some() && self.examples.is_empty())].spacing(8.0).into(),
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
                        .id("correct-transcript"),
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
                text(format!("Saved examples ({})", self.examples.len())).semibold(),
                spacer(),
                button("Export training data…")
                    .on_press(Msg::ExportExamples)
                    .enabled(idle)
            ]
            .spacing(8.0)
            .into(),
        );
        if self.examples.is_empty() {
            content.push(
                text("No saved examples for this word.")
                    .role(Role::Text3)
                    .into(),
            );
        }
        for example in &self.examples {
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
