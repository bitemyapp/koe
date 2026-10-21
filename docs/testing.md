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

For a manual session check, first run `koe doctor`. Choose a non-sensitive
text buffer, start dictation, speak, stop, and observe insertion. Repeat with
focus changed during recognition: insertion should be refused and
`koe record paste-last` should recover the transcript. Test a pause and verify
the PipeWire source closes until resumed. GPU builds and every microphone /
compositor combination require hardware-specific testing; CPU fixture tests
do not establish that coverage.
