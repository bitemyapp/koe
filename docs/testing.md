# Verification

Normal tests use temporary configuration, Unix sockets, a fixture PCM producer,
a loopback HTTP server, and a stand-in for the Aizome compositor. They neither
record the host microphone nor send keys to the host desktop. Runtime sockets,
model-loading code, process lifecycle, multipart requests, and state transitions
are real; only external audio and desktop services are replaced.

```sh
nix develop
cargo test --locked --all-targets
cargo test --locked --all-targets --no-default-features
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --no-default-features -- -D warnings
nix flake check
```

For actual offline recognition, download the pinned, checksum-verified tiny
English model and whisper.cpp's public JFK sample into a scratch directory:

```sh
mkdir -p /tmp/koe-smoke
cargo run -- model download tiny.en --directory /tmp/koe-smoke
curl -fL https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav \
  -o /tmp/koe-smoke/jfk.wav
KOE_TEST_MODEL=/tmp/koe-smoke/ggml-tiny.en.bin \
KOE_TEST_WAV=/tmp/koe-smoke/jfk.wav \
  cargo test --test core local_whisper_recognizes_jfk -- --ignored --nocapture
```

The test asserts recognition of “ask not what your country”, then verifies
resident-model unloading. It is ignored in normal CI because CI should not
download model weights or depend on an external model host.

The two-toggle desktop regression runs the installed Aizome compositor headless,
opens foot, sends Super+Alt+D twice through the real compositor, and verifies the
recognized text arrives in that terminal through the real Wayland clipboard.
It enables kitty keyboard reporting and bracketed paste, disables compositor
command-key translation, and checks bracketed text arrives without Ctrl+V.
It uses fixture speech, its own sockets, private configuration and no session
bus; it never uses the login session's microphone, clipboard, or keyboard.
It requires `aizome`, `foot`, and `wl-copy` on PATH:

```sh
KOE_TEST_MODEL=/tmp/koe-smoke/ggml-tiny.en.bin \
KOE_TEST_WAV=/tmp/koe-smoke/jfk.wav \
  cargo test --test aizome -- --ignored --nocapture
```

The ordinary daemon suite separately checks that the second toggle immediately
announces transcription, that another toggle during recognition reports the
microphone is off, and that none of these notifications contains speech.

The tray integration test starts a private `dbus-daemon` and StatusNotifierWatcher,
then invokes real DBusMenu actions. It checks registration, phase updates,
start/pause/resume/cancel, re-registration after a host restart, and clean quit.
Its microphone is a fixture process. It does not touch the login session bus:

```sh
cargo test --test tray -- --ignored --nocapture
```

The CPU package acceleration was checked using the multilingual `base` model,
automatic language selection, four inference threads, and the 11-second JFK
sample on a Ryzen 7 9800X3D. A fresh-process transcription took 11.24 seconds
with the original portable package and 2.84 seconds with OpenBLAS enabled,
with identical output. This measures one clip and includes model loading;
it is not a latency guarantee for all recordings or hardware.

The Vulkan package was also checked on this machine's RTX 5090 using the same
multilingual `base` model, automatic language selection, and 11-second sample.
After startup warmup, two consecutive dictations took 39 and 41 ms to recognize,
and 210 and 209 ms from stop to pasted text (including the default 150 ms
clipboard delay). The headless Aizome test checked actual bracketed insertion.
These are sample measurements, not a guarantee for every utterance. To repeat
against an installed Vulkan package while preserving the fixture microphone:

```sh
KOE_TEST_KOE=/path/to/package/bin/.koe-wrapped KOE_TEST_GPU=1 \
KOE_TEST_LANGUAGE=auto KOE_TEST_MODEL=/path/to/ggml-base.bin \
KOE_TEST_WAV=/tmp/koe-smoke/jfk.wav \
  cargo test --test aizome -- --ignored --nocapture
```

The unwrapped Nix executable is deliberate: the package's normal wrapper puts
the real `pw-record` first on PATH, whereas this test supplies a fixture.

For a manual session check, first run `koe doctor`. Choose a non-sensitive
text buffer, start dictation, speak, stop, and observe insertion. Repeat with
focus changed during recognition: insertion should be refused and
`koe record paste-last` should recover the transcript. Test a pause and verify
the PipeWire source closes until resumed. GPU builds and every microphone /
compositor combination require hardware-specific testing; CPU fixture tests
do not establish that coverage.
