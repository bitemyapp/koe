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
`subscribe`, `last`, `copy-last`, `paste-last`, `clear-last`, `unload`, `shutdown`.
Unknown actions and fields are rejected. `shutdown` exits the process; a
systemd service configured with `Restart=on-failure` stays stopped until started.

A rejected command has `ok:false` and a top-level `error` string. An
asynchronous capture, inference, or delivery failure appears in
`status.phase = "error"` and `status.error`; a successful `status` request
still has `ok:true`. `stop` returns after submitting recognition, not after
transcription or paste completes. Use the stream to observe completion.

The `last` response adds a `text` string. Other responses and all subscription
updates omit it. `clear-last` does not clear the desktop clipboard. `cancel`
discards the current recording/work and preserves the last recognized text,
including text retained after failed delivery.

`sequence` increases on published changes and resets at daemon restart.
Read-only `status` requests do not emit updates. `elapsed_ms` is captured audio
duration, excluding pauses. `level` is RMS amplitude in 0..1, not decibels or
speech probability. Meter updates are capped at 10 Hz; paused/idle states do
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
