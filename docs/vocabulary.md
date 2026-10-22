# Vocabulary packs and teaching examples

This first version improves vocabulary through prompt hints and approved literal
corrections. It collects explicitly saved audio/label pairs for a later training
pipeline; it does not fine-tune Whisper or load neural adapters.

## Portable vocabulary

An exported `.koe.json` is UTF-8 JSON, limited to 32 KiB. For example:

```json
{
  "format": "koe-vocabulary",
  "version": 1,
  "pack": {
    "id": "my-desktop-terms",
    "name": "Desktop terms",
    "description": "Names used by our team",
    "words": [
      {"id": "aizome", "spelling": "Aizome", "aliases": ["Isome", "IZOMA"]}
    ]
  }
}
```

IDs contain 1–64 ASCII letters, digits, hyphens or underscores. A library holds
at most 16 packs and 256 words. Written forms and aliases are trimmed, nonempty,
at most 128 bytes, and contain no control characters. Each word allows at most
16 aliases. Unknown fields, invalid IDs and oversized files are rejected.
Imports preserve pack IDs when available and start disabled. An identical pack
already installed is rejected. A changed pack with an existing ID is imported
as a separate copy with a fresh local ID and an `(imported)` name suffix; it
never overwrites existing edits or recording references. Pack files never
contain commands, file references, model weights, audio or training examples.

Enabled packs combine their written forms into an inference hint. Configured
`engine.prompt` is retained. Hints are limited to 1024 bytes and the local model's
actual token allowance (half its text context); oversized hints report an error
instead of silently dropping entries. Server-side prompt support varies in HTTP
mode. Aliases are case-insensitive whole-word matches; longer aliases run first,
followed by existing TOML replacements. Conflicting active mappings and aliases
that overwrite another active canonical spelling are rejected. A correction
applies everywhere while its pack is enabled, so avoid ambiguous common words.

The private `library.json` additionally records `version`, `revision`, `enabled`
and `packs`. Mutations lock, check the expected revision, validate and atomically
replace it. The first library starts with Personal and Sekka; removing or disabling
those packs is persistent after the first edit. The file is re-read between
dictations, while resident model weights and inference buffers are shared.

## Explicit teaching data

Teaching uses the selected word as an annotation target. A user records a short
sentence, reviews the recognition, and corrects the complete transcript. The
recording is not automatically used to infer a pronunciation rule. An isolated
text correction can be suggested, but the user must separately approve it.
The training label must describe everything audible, not just the selected word.

Recording and playback use memory. Only Save example creates
`examples/<random-id>/audio.wav` and `example.json` in Koe's private vocabulary
directory. Directories are mode 0700; files are 0600. Regular dictation is never
captured by this mechanism. Deleting an example removes its saved pair; word and
pack removal is blocked until associated examples have been deleted. Exports
already made are independent copies and are not removed with the original.

Export training data creates a **new folder**, with `audio/<id>.wav` and one
JSON object per line in `manifest.jsonl`. Each record includes:

| Field | Meaning |
|---|---|
| `version`, `id` | Format version 1 and stable example ID |
| `audio`, `audio_sha256` | Relative WAV path and SHA-256 of its complete bytes |
| `pack_id`, `word_id`, `spelling` | Annotation target at recording time |
| `transcript` | User-reviewed complete utterance, up to 4096 bytes |
| `heard` | Raw recognizer output, before literal corrections |
| `model`, `language` | Model filename or `http:<model>` and configured language |
| `vocabulary_revision` | Vocabulary snapshot used for recognition |
| `created_ms` | Save time in Unix milliseconds |
| `sample_rate`, `duration_ms` | 16000 Hz and sample-derived duration |

Audio is mono signed 16-bit PCM WAV, at most 30 seconds. Export verifies each
stored checksum and stages the complete folder before publishing it. It does
not overwrite existing destinations. This manifest is a starting dataset, not
a trained model: later training needs quality checks, diverse examples, held-out
evaluation, and conversion to the chosen trainer's input format. The model
filename is provenance, not a cryptographic identity for custom weights.
