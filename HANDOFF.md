# Koe handoff — 2026-10-21

Koe is installed and running on the user's Aizome desktop. The first native
vocabulary/teaching interface is shipped, with shareable text packs and explicitly
saved examples for later training. **No neural fine-tuning or adapter loading is
implemented.** The next product step is to evaluate this flow with the user's
speech before choosing a training approach.

This is a snapshot taken around 23:50 CDT on October 21, 2026. Runtime details and
CI status below should be rechecked when resuming work.

## Repository and delivered changes

- Local checkout: `/home/callen/work/koe`, branch `main`.
- Remote: [bitemyapp/koe](https://github.com/bitemyapp/koe),
  `git@github.com:bitemyapp/koe.git`.
- Runtime source baseline: [`fdb6513`](https://github.com/bitemyapp/koe/commit/fdb6513).
  The handoff itself is a later documentation commit; it does not require a
  binary rebuild.
- [`767f291`](https://github.com/bitemyapp/koe/commit/767f291): native teaching,
  vocabulary packs, sample storage/exports, packaging and tests.
- `fdb6513`: update the footer when recording stops, review begins, or an example
  is discarded. This fixed misleading “Recording” text during review.
- Earlier behavior worth preserving: `333a579` added the tray, terminal paste
  handling and warmed GPU inference; `be85823` waits for held shortcut keys to
  be released before pasting.

The user previously found dictation too slow, saw terminal image-paste errors,
and had confusing recording/error states. They subsequently confirmed that
ordinary dictation felt responsive enough. Their current motivation is teaching
domain names: **Aizome** was recognized as **Isome** or **IZOMA**. They explicitly
chose vocabulary packs and saved examples as the first step, before model
training. No feedback on the new teaching flow's recognition quality has arrived
yet.

## Installed desktop state

| Item | Snapshot |
|---|---|
| User service | `aizome-koe.service`, active; idle, model loaded, microphone off |
| Nix profile entry | `koe-vulkan`, from the local flake at `fdb6513` |
| Installed package | `/nix/store/3960knjl5wgncbqcy9lahn0lwskf9q4m-aizome-koe-0.1.0` |
| Executable | `/home/callen/.nix-profile/bin/koe` |
| Configuration | `/home/callen/.config/aizome/koe.toml` |
| Recognition | Local multilingual Whisper **base**, via whisper-rs/whisper.cpp |
| Weights | `/home/callen/.local/share/aizome/koe/models/ggml-base.bin` |
| Engine settings | Vulkan GPU enabled, 4 threads, automatic language detection, translation off |
| Output | Paste, same-window guard enabled, 150 ms paste delay |
| Dictation shortcut | Super+Alt+D toggles; Super+Alt+Escape cancels |

The native window was opened with Aizome selected. It runs in the temporary user
unit `koe-vocabulary-window.service`, created with `systemd-run --user --collect`;
this is not a permanent extra service. Its app ID is
`org.aizome.Koe.Vocabulary`. Normal entry points are the tray's
**Vocabulary and teaching…** item, the desktop action, and `koe vocabulary`.
Opening it does not start recording.

Useful read-only checks:

```sh
koe status
koe packs list
systemctl --user status aizome-koe.service
journalctl --user -u aizome-koe.service -n 30 --no-pager
```

Do not restart the daemon during recording, transcription, delivery, or teaching
review. Check both `status.phase` and `status.teaching`: idle + teaching means a
pending example still exists. Restarting also loses the last ordinary transcript
held in memory.

## User-facing behavior

**Packs:** Personal starts empty. Sekka starts enabled and contains Aizome, Sekka
and Koe. Its approved `Isome` and `IZOMA` aliases map to `Aizome`. Multiple packs
can be enabled together. Written forms become recognition hints; approved aliases
are case-insensitive literal word corrections. Changes apply on the next
dictation without restarting or reloading weights. Enablement is global across
the desktop, not selected by application.

**Teaching:** select a saved word → Record example → say a short sentence → Stop
recording → optionally Play sample → edit the complete transcript → Save example
or Discard. The preview distinguishes raw recognition from the corrected text.
An isolated changed word/phrase may offer **Add correction**; the user must click
to approve it. Approving a correction and saving a recording are separate actions.
Saving audio alone does not change future recognition or create an alias.

**Sharing:** Export pack writes text-only `.koe.json`. Imports start disabled.
An identical pack with the same ID is rejected; a changed pack with an existing
ID gets a fresh local ID and an `(imported)` name suffix, preserving existing
edits and sample references. Training export is a separate action that creates
a new folder of WAV files and a JSONL manifest. Exports do not overwrite paths.

See [README.md](README.md#vocabulary-and-teaching) for usage and
[docs/vocabulary.md](docs/vocabulary.md) for formats and limits.

## Architecture and invariants

| Location | Responsibility |
|---|---|
| [src/ui.rs](src/ui.rs) | Aizome Kit window, pack/word editing, file panels, review/editor/playback controls, approved correction suggestions, close cleanup |
| [src/vocabulary.rs](src/vocabulary.rs) | Pack validation, combined hints/corrections, revision-checked storage, import/export, explicit samples and dataset export |
| [src/daemon.rs](src/daemon.rs) | Serialized recording state, teaching ownership/lease, per-session vocabulary, inference and cleanup |
| [src/engine.rs](src/engine.rs) | Local/HTTP recognizer; per-utterance prompts share resident Whisper context and inference buffers |
| [src/protocol.rs](src/protocol.rs), [src/ipc.rs](src/ipc.rs) | Versioned requests, vocabulary replies and private socket transport |
| [src/audio.rs](src/audio.rs) | PipeWire capture and memory-only sample playback through `pw-play` |
| [src/tray.rs](src/tray.rs), [src/main.rs](src/main.rs) | Tray/window entry points, recording controls, pack CLI and file transcription |
| [src/desktop.rs](src/desktop.rs) | Focus/lock/held-key guards, clipboard delivery and correct terminal paste shortcuts |

- Ordinary dictation does not save audio or transcript history. Its last result
  stays in memory for explicit retrieval/retry.
- Teaching has an owner token and a separate buffer. It never pastes, changes the
  clipboard, or replaces the last ordinary transcript. At review, the microphone
  is off even though `status.teaching` remains true.
- Teaching recordings are capped at 30 seconds, or a lower configured recording
  limit. Silence auto-submission is disabled for teaching.
- The window polls every 500 ms. A session is cancelled after more than eight
  seconds without an owner heartbeat, checked on a one-second watchdog. Normal
  close actively cancels; it does not wait for the lease to expire.
- Only TeachSave persists a WAV/label pair. Cancel, close, shutdown and lease
  expiry discard unsaved audio. Playback streams from memory without an audio
  temporary file.
- Status broadcasts, notifications and ordinary logs do not carry transcripts
  or teaching tokens. Explicit teaching requests can retrieve preview text.
- Vocabulary writes use a lock, revision check and atomic replacement. Stale
  edits report an error and require Refresh. Removing words/packs with saved
  examples is blocked until those examples are deleted.
- Pack/file work is dispatched outside the recording actor. The UI uses workers
  for IPC and keeps its editor responsive. Long training exports have a longer
  client timeout.
- Model weights and inference buffers stay resident. Do not reintroduce a
  per-recording model load or state allocation when adding personalization.

Data lives under `$XDG_DATA_HOME/aizome/koe/vocabulary`, normally
`~/.local/share/aizome/koe/vocabulary`: `library.json` plus
`examples/<id>/{audio.wav,example.json}`. Directories are 0700 and files 0600.
Training exports include complete corrected utterances, raw recognition,
model/language, target word/pack, vocabulary revision, timing and audio hashes.
Do not commit or automatically upload user recordings or labels.

Protocol details are in [docs/protocol.md](docs/protocol.md). These are additive
version-1 extensions: action `vocabulary`, its command payload, optional response
`vocabulary`, and `status.teaching`.

## Build and integration notes

- Rust 1.95+; default features are `local` and `ui`. A CLI-only local build uses
  `--no-default-features --features local`. A no-default-features build can use
  the HTTP backend and pack CLI, but has no native window or local recognizer.
- Aizome Kit is pinned to `e012286df2199d2e8b0fd2e9a951835b2a8a6494`.
  Its workspace Sidestep dependency is resolved with a Cargo patch to
  `dae957e913376a26e642d6abee533eb755fb9092`. Update Cargo pins, lockfile and Nix
  git-source hashes together; do not depend on an adjacent mutable checkout.
- The tiny-skia dev-profile overflow-check exception is needed for its SIMD
  implementation. The UI uses native Aizome Kit, not GTK or a web view.
- Nix packages include OpenBLAS; `koe-vulkan` additionally enables Vulkan.
  The wrapper supplies PipeWire, wl-clipboard, wtype, Fontconfig and Wayland.
- `WHISPER_DONT_GENERATE_BINDINGS=1` works for ordinary CPU source builds.
  Vulkan needs regenerated bindings, libclang, Vulkan headers/loader and glslc;
  the Nix Vulkan package handles this automatically.
- `PrivateTmp=false` is intentional in both service definitions: the daemon and
  native file dialogs must resolve imports/exports to the same desktop paths.
  Private data permissions, UMask 0077, core-dump suppression and
  NoNewPrivileges remain in place.
- This feature changed Koe only; no Aizome/Settei repository patches were made.
  A future Settei link can launch `koe vocabulary`.
- The user authorized `ssh wx-workstation` for CPU-heavy work if local CPU time
  is contended by other agents. Local builds used four jobs during this work.

For a future source update on this machine:

```sh
cd /home/callen/work/koe
nix build .#koe-vulkan --cores 4 --max-jobs 1
# After verification and committing the intended source:
nix profile upgrade koe-vulkan --cores 4 --max-jobs 1
systemctl --user daemon-reload
koe status
# Restart only after confirming no active dictation or teaching session:
systemctl --user restart aizome-koe.service
koe vocabulary
```

## Verification evidence

The installed Vulkan package passed its full bundled suite: **34 tests**, with
three environment-dependent tests ignored by the normal suite. Default and
no-default-features Rust tests and Clippy checks passed. The isolated tray test
and real headless Aizome/foot dictation regression were also run explicitly.

The native Kit test drives the actual window against a fixture daemon through
recording, transcript editing, alias approval, playback, explicit save and
vocabulary-only export. Other tests cover stale writes, import collisions,
disabled imports, invalid input, WAV metadata/checksums, deletion, ownership,
lease expiry, hot reload and absence of incidental audio/clipboard writes.

On the vocabulary feature build, the 11-second public JFK fixture took 211 ms
then 44 ms to recognize on the RTX 5090. Stop-to-paste was 376 ms and 440 ms;
the second deliberately held the shortcut through recognition for another
200 ms. On the exact installed build, a separate native Wayland/GPU teaching
smoke reached review with 216 ms recognition and discarded unsaved audio on
window close in 41 ms, both during recording and review. These are fixture
measurements, not measurements of the user's Aizome pronunciation accuracy.

CI for both [`767f291`](https://github.com/bitemyapp/koe/actions/runs/39723382282)
and [`fdb6513`](https://github.com/bitemyapp/koe/actions/runs/39723717584)
passed all jobs: both Rust feature matrices and Nix. The exact installed Vulkan
derivation also passed locally. CI's Nix job checks the default CPU
package/module; it does not establish GPU hardware coverage.

Repeatable commands and fixture isolation are documented in
[docs/testing.md](docs/testing.md). In particular:

```sh
nix develop
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --all-targets --no-default-features
cargo test --test tray -- --ignored --nocapture
KOE_TEST_SCREENSHOTS=/tmp cargo test --test daemon native_window -- --nocapture
```

Use `tests/aizome.rs` with `KOE_TEST_MODEL`, `KOE_TEST_WAV`, `KOE_TEST_GPU=1`
and `KOE_TEST_KOE=/path/to/package/bin/.koe-wrapped` for the headless GPU regression.
The unwrapped executable preserves the fixture microphone first on PATH. Do not
send synthetic keystrokes or fixture paste into the user's live desktop.

Temporary evidence may still be available on this workstation:

- `/tmp/koe-vocabulary-installed.log`: final Nix package build and tests.
- `/tmp/koe-vocabulary-gpu.log`: headless two-toggle/held-key regression.
- `/tmp/koe-native-teaching-smoke.pl` and its `-final.log`: one-off real Wayland
  window/GPU/close checks. The script uses private runtime/data directories,
  fixture PCM and an isolated compositor; it is **not committed or a CI test**.
- `/tmp/koe-vocabulary.png`, `/tmp/koe-teaching-review.png`, and
  `/tmp/koe-teaching-native.png`: inspected fixture-only UI renders.
- `/tmp/koe-research/jfk.wav`: the public speech fixture.

Do not rely on `/tmp` surviving. Promote the one-off native close test into an
ignored repository integration test if it becomes part of regular validation.

## Limits and next work

1. **Get real-user feedback first.** Check Aizome in varied sentences and normal
   desktop dictation. Compare raw recognition with final corrected text, check
   false corrections on unrelated words, and preserve the user's accepted
   latency. Retain recordings only through explicit Save example.
2. **Improve pack UX as evidence warrants.** There is no per-app activation,
   in-place pack update/merge flow, pronunciation lexicon, or forced alignment.
   Alias suggestions require a single isolated changed span; manual aliases
   handle other cases. Aliases use Unicode regex word boundaries, so punctuation
   in domain terms needs separate evaluation. Very large hint lists are bounded;
   the model-specific token limit is checked at recognition time, not in the UI.
3. **Prepare data before training.** Validate complete labels/audio, duplicates,
   duration and quality; keep evaluation examples separate from training. Current
   provenance has a model filename, not a weight hash, and has no speaker/session
   split metadata or review-quality score. Design additions and migrations before
   depending on those fields in a trainer.
4. **Prototype a compatible training/runtime path separately.** Koe currently
   has no acoustic adaptation or adapter-loading API. Do not promise that LoRA
   layers can be arbitrarily stacked or swapped in the current whisper.cpp path.
   Establish runtime/export compatibility, domain-word accuracy, general-speech
   regressions and latency before exposing trained adapters in this UI. Portable
   vocabulary packs should remain useful independently of the chosen model.

There is no known implementation blocker for trying the shipped teaching flow.
The main unanswered question is how well hints and explicit corrections handle
the user's actual speech, and whether enough reviewed data warrants acoustic
training afterward.
