# Koe control protocol, version 1

Unix stream: `$XDG_RUNTIME_DIR/aizome-koe/control.sock`. CLI overrides with
`--socket PATH`; its parent must be private (0700) and owned by the user.
A lock file enforces one daemon. Stale sockets are removed only after taking
the lock. Socket permissions are 0600.

One UTF-8 JSON object per newline, at most 64 KiB including the newline.
Each connection sends one request and receives one response. A subscriber
then receives state changes until disconnected. Requests without `version`
default to 1; incompatible versions receive a structured error. Malformed,
oversized, or incomplete requests are disconnected. Connections without a
request time out after three seconds; blocked writes also time out. There
are at most 64 concurrent connections and 32 queued commands.

```json
{"version":1,"action":"toggle"}
```

```json
{"version":1,"ok":true,"status":{"phase":"recording","sequence":1,"elapsed_ms":0,"level":0.0,"has_last":false,"model_loaded":false,"error":null}}
```

Actions: `start`, `stop`, `toggle`, `pause`, `resume`, `cancel`, `status`,
`subscribe`, `last`, `copy-last`, `paste-last`, `clear-last`, `unload`, `shutdown`,
`vocabulary` (with a `vocabulary` command payload).
Unknown actions and fields are rejected. `shutdown` exits the process; a
systemd service configured with `Restart=on-failure` stays stopped until started.

A rejected command has `ok:false` and a top-level `error` string. An
asynchronous capture, inference, or delivery failure appears in
`status.phase = "error"` and `status.error`; a successful `status` request
still has `ok:true`. `stop` returns after submitting recognition, not after
transcription or paste completes. Use the stream to observe completion.
`paste-last` also acknowledges submission: it enters `delivering`, then `idle`
or `error`. It is accepted only while idle or in error and can be cancelled.
On Aizome, delivery waits up to five seconds for held keys to be released,
checking focus and lock state throughout the wait.

The `last` response adds a `text` string. Ordinary responses and all subscription
updates omit it. Explicit vocabulary/teaching requests can return reviewed text
in a separate `vocabulary` object; it is never broadcast. `clear-last` does not
clear the desktop clipboard. `cancel`
discards the current recording/work and preserves the last recognized text,
including text retained after failed delivery.

`sequence` increases on published changes and resets at daemon restart.
Read-only `status` requests do not emit updates. `elapsed_ms` is captured audio
duration, excluding pauses. `level` is RMS amplitude in 0..1, not decibels or
speech probability. `transcription_ms` is the last recognition attempt's wall
time (null until one completes), excluding capture and clipboard delivery.
`model_loaded` indicates resident model buffers, prepared at daemon startup.
Meter updates are capped at 10 Hz; paused/idle states do
not generate periodic events. Subscribers receive the latest snapshot and
may miss intermediate states if they are slow. The subscription is a UI
state stream, not an audit log.

The daemon may return `busy` for an incompatible action. Repeated `start`
or `stop` is rejected; `toggle` is only valid while idle/error or recording/
paused. A start during transcription is not queued. Cancel first to discard
it. A pause closes the microphone before acknowledging the request.

All interaction is local to the Unix user. The protocol is not a security
boundary between processes of that user. Consumers should tolerate future
response fields and report a version mismatch rather than guessing its meaning.

## Vocabulary and teaching

`{"action":"vocabulary","vocabulary":{"command":"library"}}` retrieves the
library and recognition-location description. `save-library` takes a complete
`library` including the revision read; stale edits are rejected. Import/export
and example commands are defined by `VocabularyRequest` in `src/protocol.rs`.
File paths are absolute local paths, not URLs. Store work runs separately from
the recording actor. Training export clients allow two minutes for file copies.

`teach-start` takes `token` (a random session owner ID), `pack` and `word`. It
starts a bounded recording with no paste target. Use `teach-stop` to recognize,
then `teach-poll` to retrieve `{heard, corrected, duration_ms}`. Each command
requires the same token. The owner must poll at least every eight seconds while
recording, recognizing, or reviewing, or the daemon discards the session.
The native window polls twice a second. `teach-play` plays the in-memory sample;
`teach-save` takes the complete corrected `transcript` and explicitly persists
the pair. `teach-cancel`, global `cancel`, shutdown, and lease expiry discard
unsaved audio. Saving clears the session. These commands never replace the last
ordinary transcript or write to the clipboard.

`status.teaching` is true throughout this flow, including `phase:idle` during
review (microphone off). Consumers must not offer a new dictation while teaching
is true. Subscription snapshots never include a session token, audio, previews,
vocabulary content or saved examples. See [vocabulary.md](vocabulary.md) for the
portable pack and explicit training export formats.
