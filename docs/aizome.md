# Integrating Koe into Aizome

Koe is a separate Rust service and repository. It depends on Aizome's published
JSON protocol, not its internal crates or the compositor's reloadable ABI.
It does not depend on Hyprland, evdev, GTK, Qt, X11, or root privileges.

## Working integration

- `programs.aizome.koe.enable` adds the package and a user service tied to
  `aizome-session.target`. The compositor imports the Wayland and Aizome
  socket environment before the service starts. Models remain user data,
  not part of the system closure or an activation-time download.
- `org.aizome.Koe.desktop` makes Dictation discoverable in Aizome's launcher.
  It starts the session service (or a background daemon when no unit is installed)
  without recording, and reuses an existing daemon. Its desktop actions toggle,
  cancel, copy, and forget.
- The daemon exports `org.kde.StatusNotifierItem` and `com.canonical.dbusmenu`
  on the session bus using `ksni`. Aizome's existing systray consumes the icon,
  phase-dependent menu, tooltip, and attention status. Menu commands use Koe's
  serialized control socket. No Aizome UI patch is required for the tray.
- Compositor `[keys.bindings]` actions invoke short-lived CLI clients.
  Koe never opens input devices or intercepts application keys.
- Notifications use the actual shell `notify` request, with `app = "Koe"`.
  They announce listening, transcription, completion and actionable failures,
  never transcript contents. The current shell's OSD
  supports volume/brightness, not a dictation visualizer; Koe does not pretend
  that a new OSD kind already exists.
- Paste goes through `wl-copy`, then compositor `synthesize`: `super+v` for
  native kit apps, `ctrl+shift+v` for recognized terminals, `ctrl+v` otherwise.
  These are actual modifier events; they do not rely on the compositor's
  command-key translation. `output.paste_shortcut` overrides automatic selection
  for custom terminal app IDs or bindings.

Before capture, Koe saves the focused window ID. Before clipboard replacement
and again immediately before paste, it checks that the same window still has
focus, the session is unlocked, and no keys remain pressed. It never steals
focus back. If a check fails, `paste-last` offers explicit recovery in the
current window. These are best-effort checks: existing compositor APIs do
not provide an atomic “paste into this window if still focused” transaction.
A focus change between the final check and input remains possible.

## Deskbar and Settei contract

A native shell consumer should connect to `$XDG_RUNTIME_DIR/aizome-koe/control.sock`
and send `{"version":1,"action":"subscribe"}`. The initial snapshot and later
updates have the same schema; there is no polling. See [protocol.md](protocol.md).

Suggested shell presentation:

| State | Display | Action |
|---|---|---|
| idle | optional microphone icon | start |
| recording | microphone with RMS meter and elapsed time | stop; secondary cancel |
| paused | paused microphone and retained duration | resume; secondary stop/cancel |
| finishing | finishing indicator | cancel |
| transcribing | indeterminate progress | cancel |
| delivering | brief completion indicator | cancel before paste is sent |
| error | attention state with diagnostic | retry; copy/paste last when available |

Use the shell's own theme tokens and icons. Update its meter on the 10 Hz
status stream, and redraw only the affected surface. Do not add a rendering
loop to the daemon or display a percentage for model inference: no honest
percentage is available. Slow subscribers get the newest state, not an
unbounded queue of old levels. Reconnect with backoff after a service restart.

Settei can edit the TOML configuration or expose declarative settings in a
Sekka system configuration. Important controls are backend, model, language,
microphone source, paste versus clipboard, and optional silence submission.
A server selection must clearly say audio is sent to that server. Vocabulary
prompts and overrides are ordinary user settings, not credentials. Apply
configuration changes by restarting the service while idle.

Koe's native `koe vocabulary` window uses Aizome Kit and can be opened from
the tray or a Settei link. Pack changes apply to the next dictation without
restarting the service. `status.teaching` distinguishes a teaching recording
or review from ordinary dictation; idle + teaching means microphone off and
waiting for Save/Discard. Do not offer Start in that state. See
[vocabulary.md](vocabulary.md) for pack and training-data contracts.

These Deskbar/Settei UI changes are **integration work for their repositories**;
Koe supplies the working transport and commands, not an unmerged UI patch.

## Scope relative to hyprwhspr

| Capability | Koe 0.1 |
|---|---|
| Toggle, start/stop, cancel | implemented |
| Pause/resume long-form dictation | implemented, memory only |
| Local multilingual Whisper, English translation | implemented |
| Cached model and explicit unloading | implemented |
| Language, prompt and word overrides | implemented |
| Optional HTTP recognition | implemented, multipart/JSON contract |
| Verified model manager, file transcription | implemented |
| Native Aizome clipboard/paste, notifications, session lifecycle | implemented |
| Shell status/meter stream, Waybar adapter | implemented |
| System tray status, menu, shell restart recovery | implemented |
| Vulkan package, resident inference buffers and startup warmup | implemented |
| Native vocabulary/teaching window, shareable packs | implemented |
| Explicitly saved recordings and corrected labels for future training | implemented |
| Acoustic fine-tuning and interchangeable model adapters | not implemented |
| Graphical setup/settings and animated overlay | not implemented |
| Parakeet, Qwen, Cohere, ElevenLabs-specific adapters | not implemented |
| Partial transcript streaming and live typing | not implemented |
| Audio ducking, neural VAD, automatic recording history | not implemented |
| X11, direct global hotkey capture | not implemented |

## Architecture

`config` validates typed settings; `audio` negotiates a mono 16 kHz stream
through `pw-record` and decodes PCM boundaries; `engine` embeds Whisper or
calls the explicitly configured server; `text` normalizes and replaces words;
`desktop` delivers and checks focus; `daemon` owns the state machine; `ipc`
bounds and frames requests. The binary is only the CLI/front door.

One actor serializes state transitions. Capture and recognition run outside
it. A paused session keeps its samples but closes the capture process.
Recognition cancellation both drops the async job and signals Whisper's
abort callback; a model load already in progress may finish before noticing
the flag. Inference contexts are serialized and cached. A cancelled worker
cannot deliver a late result into a later recording. HTTP requests and
pending delivery are dropped on cancellation. Once a paste chord has been
sent, cancellation cannot retract text already delivered to an application.
