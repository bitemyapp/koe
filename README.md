# Koe 声

**Voice dictation for Aizome, Sekka's desktop.** Press ❖⌥D, speak, press it
again: your words appear in the focused application.

Koe is a Rust reimplementation of the core
[hyprwhspr](https://github.com/goodroot/hyprwhspr) workflow, built around
[Aizome](https://github.com/bitemyapp/aizome) instead of Hyprland. It runs as a
small session service. Whisper inference is embedded through `whisper-rs`
and whisper.cpp; there is no Python runtime or separate inference server
in the default configuration.

- **Local by default.** Microphone audio stays in memory; no recordings,
  transcript history, telemetry, or automatic model downloads.
- **Native Aizome input.** Paste uses the compositor's IPC and ❖V, including
  its command-key translation for ordinary applications and terminals.
  Focus and lock checks prevent delivery to a different window during
  recognition. The last transcript remains available if delivery fails.
- **A complete dictation loop.** Toggle or start/stop, pause/resume with the
  microphone closed during pauses, cancel, replay/copy/forget the last
  transcript, optional silence submission, and bounded recording length.
- **Local models and vocabulary.** Multilingual Whisper, translation to
  English, language selection, prompts, literal word overrides, resident
  model reuse, explicit unloading, and pinned SHA-256 model downloads.
- **Ready for the shell.** Versioned Unix-socket control and event-driven
  status with microphone levels, native Aizome notifications, desktop
  actions, NixOS integration and an Aizome session unit. The JSON stream is
  also usable by Waybar.
- **Optional HTTP recognition.** Explicitly select a compatible multipart
  transcription server, with environment-sourced credentials and bounded
  requests. This mode sends audio to the configured server.

This is an initial implementation, **not full hyprwhspr feature parity**.
It does not yet include a graphical settings window, waveform overlay,
Parakeet/Qwen backends, streaming partial transcripts, audio ducking, or
X11 support. The Aizome Deskbar and Settei consumer contracts are documented;
this repository does not change those applications. See
[scope and integration](docs/aizome.md).

## Install on Sekka / NixOS

Add the flake and module to your system configuration:

```nix
# flake.nix inputs
koe.url = "github:bitemyapp/koe";
koe.inputs.nixpkgs.follows = "nixpkgs";

# in your nixosSystem modules
modules = [
  koe.nixosModules.default
  {
    programs.aizome.koe.enable = true;
    # Optional: nonempty settings are used instead of the user's TOML.
    # programs.aizome.koe.settings.engine.language = "ja";
  }
];
```

The module starts Koe with `aizome-session.target` and stops it with the
session. It installs the runtime tools and does not download a model or
change your key bindings. After rebuilding, log into Aizome, then:

```sh
koe model download base
koe config init                     # optional; preserves existing files
koe doctor
```

Merge [data/aizome-bindings.toml](data/aizome-bindings.toml) into
`~/.config/aizome/settings.toml` and run `aizomectl reload-config`:

```toml
[keys.bindings]
"Super+Alt+D" = "spawn koe record toggle"
"Super+Alt+Escape" = "spawn koe record cancel"
```

If enabling it within an existing session, start it with
`systemctl --user start aizome-koe`. Use `koe status` to inspect it and
`journalctl --user -u aizome-koe` for service diagnostics.

To try the package without enabling a service:

```sh
nix run github:bitemyapp/koe -- model download base
nix run github:bitemyapp/koe -- daemon
# In another terminal:
nix run github:bitemyapp/koe -- record toggle
```

## Build from source

Linux, Rust 1.88+, a C/C++ toolchain and CMake are required. At runtime,
install PipeWire's `pw-record` and `wl-clipboard`. `wtype` is needed only
for automatic paste on other Wayland desktops; those compositors must
support its virtual-keyboard protocol.

```sh
nix develop                         # or install the dependencies yourself
WHISPER_DONT_GENERATE_BINDINGS=1 cargo build --release --locked
cargo install --path . --locked     # inside nix develop
koe model download base
koe daemon
```

`WHISPER_DONT_GENERATE_BINDINGS=1` uses whisper-rs's supplied bindings.
Without it, also install libclang. The application is Rust; whisper.cpp's
inference kernels are C/C++. Capture uses PipeWire's `pw-record` for
negotiation and resampling rather than linking the application to a
particular PipeWire ABI.

For a manually installed session service:

```sh
mkdir -p ~/.config/systemd/user
cp data/aizome-koe.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now aizome-koe.service
```

The user manager must have `koe` in its executable search path. If installed
outside it, set `ExecStart` to the binary's absolute path. The Nix package
and module do this automatically.

The default CPU build uses four inference threads. Optional
`--features vulkan` or `--features cuda` builds require the corresponding
SDK, plus `engine.gpu = true`. These accelerator builds are not covered by
the CPU CI. The Nix package disables host-specific CPU instructions so a
build can be distributed to other machines.

## Use

```sh
koe record toggle                   # start; next invocation submits
koe record start                    # bind press/release to start/stop for PTT
koe record pause                    # closes microphone, keeps captured speech
koe record resume
koe record stop                     # transcribes all captured segments
koe record cancel                   # discard pending work; keep previous transcript
koe record last                     # explicitly print the last transcript
koe record copy-last
koe record paste-last               # paste into the currently focused window
koe record clear-last
koe model unload                    # release resident model memory while idle
koe status
koe watch                           # newline-delimited JSON status stream
```

Aizome's current binding configuration supports toggle actions; press/release
push-to-talk requires a hotkey host that provides both edges. There is no
global input-device listener and no need for membership in the `input` group.

File transcription prints text without changing the clipboard:

```sh
ffmpeg -i recording.mp3 -ar 16000 -ac 1 -c:a pcm_s16le speech.wav
koe transcribe speech.wav
koe transcribe speech.wav --model /path/to/ggml-small.bin
```

Files must be mono 16 kHz WAV, signed 16-bit PCM or 32-bit float. The default
five-minute limit also applies to files; raise `audio.max_seconds` for longer
input (up to one hour).

Models live under `$XDG_DATA_HOME/aizome/koe/models` (normally
`~/.local/share/aizome/koe/models`). The catalog includes `tiny`, `base`,
`small`, and their `.en` variants. `base` is the multilingual default;
`.en` models only recognize English. Use `koe model list`,
`koe model download tiny.en`, and `koe model verify tiny.en`.
Downloading another model does not change `engine.model`. Other compatible
GGML Whisper models can be selected by path.

## Configuration and privacy

Koe reads `$XDG_CONFIG_HOME/aizome/koe.toml` (normally
`~/.config/aizome/koe.toml`), or the file passed with `--config`. Missing
fields use defaults; unknown fields and invalid bounds are rejected.
`koe config show` prints the effective configuration. See the
[annotated example](data/koe.example.toml). Restart the service to apply changes.

`output.mode` is `paste`, `clipboard`, or `none`. Automatic desktop selection
uses `AIZOME_SOCKET` when present, otherwise `wtype` with Ctrl+V. For terminals
on other desktops, use clipboard mode and your normal paste shortcut.
Clipboard contents are replaced on successful delivery and are not restored;
your desktop clipboard history may retain them. `none` leaves text only in
Koe's memory for explicit retrieval.

Audio is capped and held in process memory. The last successful transcript
is held in memory until replaced, cleared, or the daemon exits. Neither is
written to disk by Koe. Memory may be swapped by the OS. The supplied service
disables core dumps. Status subscribers and notifications receive no transcript
text. No Enter key is synthesized.

HTTP mode is opt-in. It accepts a full endpoint URL, sends a multipart WAV
`file` and `model`, optional `language` and `prompt`, and `translate=true`
when enabled, then expects a JSON `{"text":"…"}` response. Support for these
fields depends on the server; this is not a universal provider adapter.
Only HTTPS or loopback HTTP is accepted. Redirects are refused. Put the
credential in the environment variable named by `engine.http.api_key_env`;
the Nix module's `environmentFile` option can load a user-private file.
Never put credentials into declarative Nix settings.

The control socket is private to your Unix user, inside a mode-0700 runtime
directory. Same-user processes can control dictation or explicitly request
the last transcript. No network server is exposed.

## Development and verification

```sh
nix develop
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo test --locked --all-targets --no-default-features
nix flake check
```

Tests exercise audio framing, configuration validation, model integrity,
bounded IPC, the daemon through real Unix sockets, HTTP multipart recognition,
pause/resume, cancellation, capture errors, clipboard delivery, and Aizome
focus guards. Capture and desktop fixtures are isolated from the login session.
No model or network download is needed for the normal test suite.

An opt-in test runs real local Whisper inference on whisper.cpp's public JFK
sample; see [docs/testing.md](docs/testing.md). Protocol consumers should start
with [docs/protocol.md](docs/protocol.md).

## Credits and license

MIT. Koe's implementation is original Rust code. The workflow and feature
reference are [goodroot/hyprwhspr](https://github.com/goodroot/hyprwhspr),
MIT, copyright 2025 goodroot. Thanks to its authors and contributors.
See [NOTICE](NOTICE) for the reference revision and the upstream license.
`whisper-rs`, whisper.cpp, models, and other dependencies retain their own
licenses. A model download does not transfer ownership or change its license.
