# Architecture

How Claude Notch is built, and the decisions behind it. Product requirements: [PRD.md](PRD.md).

## Overview

One process: a Tauri 2 app with a Rust core and a plain HTML/CSS/JS page (no bundler).

```
 ┌──────────────────────────────── claude-notch (one process) ─────────────────────────────────┐
 │                                                                                              │
 │  microphone ─cpal→ audio.rs ─16 kHz→ vad.rs ─utterances / partials→ stt.rs (whisper.cpp)     │
 │                                                                      │ text                  │
 │                                                                      ▼                       │
 │  ipc.rs (Unix socket) ──commands──▶ app.rs (driver) ◀──events── machine.rs (state machine)    │
 │        ▲                              │      ▲                                               │
 │        │ permission requests          │      └── brain.rs ── spawns ──▶ claude -p (stream-json)
 │  approve.rs (MCP server, started      │                                       │               │
 │  by Claude Code) ◀────────────────────┼───────────────────────────────────────┘               │
 │                                       ▼                                                      │
 │                       WebView: ui/index.html (the notch) · ui/settings.html (settings)       │
 │                       island.rs: layer-shell overlay, input region = the pill                │
 └──────────────────────────────────────────────────────────────────────────────────────────────┘
```

## Modules

| File | Role |
|---|---|
| `src/machine.rs` | The state machine: events in, effects out, no I/O and no clock of its own. Every behaviour is a unit test here. |
| `src/app.rs` | The driver: carries out the machine's effects (UI, Claude runs, approvals, microphone), signals, restart. |
| `src/audio.rs` | Default input device → mono 16 kHz (averaging resampler) → segmenter. Pausing closes the device. |
| `src/vad.rs` | Energy detector with an adaptive noise floor: utterances, plus a snapshot every ~1 s while someone speaks (live captions). |
| `src/stt.rs` | whisper.cpp: English, or detection limited to `auto_languages`; a prompt with the names it misspells; `audio_ctx` sized to the audio, dedicated GPU preferred. |
| `src/tts.rs` | Spoken answers: espeak-ng (en-us) phonemes in Kokoro's spelling, Kokoro-82M through ONNX Runtime (CUDA in the `tts-cuda` build, CPU otherwise), `pw-play`. |
| `src/wake.rs` | Wake phrase, cancel words and recognition noise, on text. |
| `src/brain.rs` | Runs `claude -p` in the voice workspace: prompt through stdin, stream-json parsing, cancellation per process group, connectivity probe. |
| `src/claude_settings.rs` | Reads and merges Claude Code's settings files. |
| `src/approve.rs` | MCP server behind `--permission-prompt-tool`: forwards each request to the notch. |
| `src/ipc.rs` | `$XDG_RUNTIME_DIR/claude-notch.sock`: CLI commands, permission requests, single instance. |
| `src/island.rs` | Full-screen transparent layer-shell surface whose input region is only the pill; tells the page when the pointer enters or leaves it; XWayland fallback on GNOME. |
| `src/settings.rs`, `src/history.rs` | Settings window commands; history read from Claude Code's transcripts. |
| `src/config.rs` | The notch's own `config.toml`, XDG paths, atomic writes. |
| `ui/sprite.js`, `ui/sfx.js`, `ui/position.js`, `ui/settings-model.js` | Pure, unit-tested parts of the pages: the character, sounds, dragging, settings logic. |

## State machine

```
Sleep ◀── 2 min idle ── Idle ◀────────────────────── Done / Error (shown a few seconds)
  │ speech                │ wake phrase / click / shortcut             ▲
  └────────▶ Idle         ▼                                            │
                      Listening ── utterance ──▶ Transcribing ──▶ Thinking ──tool──▶ Working
                                                                    │                 │
                                                       Approval ◀── permission prompt ┘
```

With `speak` on, an answer is read aloud with the microphone off (the notch must not hear itself
and take its own words for a follow-up); it stays on screen until the reading ends, and the
follow-up window opens then. A click, ✕, pause or a new command stops it.

At rest (idle, sleep, paused) the page tucks the pill into the top edge; a few invisible pixels
stay on screen as the input region, so hovering the edge brings it out.

During the follow-up window after an answer, speech counts as a command without the wake phrase.
Every command continues the same Claude conversation (`--resume`) until the notch restarts. If
that conversation cannot be loaded, Claude Code fails before reporting a session, and the
command is rerun in a new conversation.

## Where settings live

| What | File | Owner |
|---|---|---|
| Model, effort, permission mode, `permissions.allow` for every session | `~/.claude/settings.json` | Claude Code (user scope) |
| The same keys for voice commands only | `<workspace>/.claude/settings.json` | Claude Code (project scope of the voice workspace) |
| Personality, preferences | `<workspace>/CLAUDE.md` | Claude Code memory |
| Claude in Chrome on/off | `~/.claude.json` → `claudeInChromeDefaultEnabled` (read only) | Claude Code (`/chrome`) |
| Wake phrases, speech recognition, microphone, notch position | `~/.config/claude-notch/config.toml` | Claude Notch |
| History | `~/.claude/projects/<workspace path>/*.jsonl` | Claude Code |

`<workspace>` is `~/.local/share/claude-notch/workspace`.

Claude Notch passes no `--model`, `--effort`, `--permission-mode` or `--allowedTools`. It adds only:
- `--chrome` when the user enabled Claude in Chrome by default, because Claude Code does not apply that choice to `-p` runs;
- `--permission-mode auto` when the inherited mode is `bypassPermissions`.

Writing Claude Code's settings happens only on **Save** in the settings window: only the managed
keys whose value changed, merged into the file as it is on disk at that moment. The write is
re-read and checked just before it replaces the file, is atomic, and keeps the file mode. A backup
is kept in `~/.local/share/claude-notch/backups/`. A file that is not valid JSON is never touched.

## Security model

- The prompt reaches Claude through stdin, never argv. `--resume` ids must be UUIDs.
- The IPC socket is 0600 inside the 0700 runtime directory, and the peer's UID is checked.
- Permission requests not caused by one of the user's commands are denied. Pending requests are denied when a run ends, is cancelled, or times out.
- The pages only render text (`textContent`, enforced by ESLint), under a strict CSP, with the minimum Tauri capabilities.
- The voice workspace's default allowlist contains no generic launcher (`bash`, `setsid`, `konsole -e`).
- Downloaded speech models are checked against pinned SHA-256 sums.

## Measured decisions

These are the choices that experiments on real hardware and real Claude Code output settled.

| Finding | Consequence |
|---|---|
| In `claude -p`, a user-level `defaultMode` applies; project files cannot enable `auto` or `bypassPermissions` | Auto mode comes from the user scope; the voice scope offers the other modes |
| Project-scope `effortLevel: "low"` overrides a per-model `effortLevel: "high"` saved by `/effort` | The voice workspace ships with effort `low` |
| `claudeInChromeDefaultEnabled` gives 0 browser tools in `-p`; `--chrome` gives 22 | The notch passes `--chrome` when that setting is on |
| Resuming a missing session yields `result/error_during_execution` without `init` | Fall back to a new conversation |
| Whisper over ~100 languages heard accented English as another language and translated it | English by default; `auto` only chooses among `auto_languages` |
| A prompt made only of names produced "Chroude" (Claude + Chromium) | The prompt is a sentence |
| `small-q5_1`: 1.3 s on CPU but weak language detection; `large-v3-turbo-q5_0`: 6.7 s on CPU, 0.46 s on the GPU | Recommend turbo with the Vulkan build |
| whisper.cpp takes the first GPU of any kind (the Intel iGPU on a laptop) | Pick the first dedicated GPU |
| Background music fed 30 s chunks to Whisper (361 % CPU) | Peek at the first 3 s for the wake phrase; 12 s cap on follow-ups |
| `flash_attn` saved ~120 MB but garbled short commands | Left off |
| Kokoro fp32, 3.6 s of speech on a Quadro T1000: 0.32 s with the CUDA provider, 0.81 s on the CPU. WebGPU (Vulkan) was slower than the CPU (1.37 s for 4 s). The fp16 model gave NaN audio on both WebGPU and CUDA | fp32 model; CUDA provider in the `tts-cuda` build, CPU otherwise |
| With the input region only around the pill, WebKitGTK kept the pill hovered after the pointer left it, and sent nothing when the pointer jumped straight back | The page hears enter and leave from GTK's crossing events (`pointer`) |
| espeak-ng drops punctuation, and Kokoro then reads without pauses | Each clause is phonemized alone and its mark put back |
| Default systemd `KillMode` killed apps opened by voice | `KillMode=process`, and the notch stops its own `claude -p` on SIGTERM |

## Build and release

- `scripts/check.sh`: rustfmt, clippy (pedantic, `-D warnings`), Rust tests, cargo audit, ESLint, UI tests, npm audit, shellcheck, systemd unit and desktop entry validation. `--slow` adds end-to-end speech tests (espeak-ng → whisper).
- `.github/workflows/ci.yaml`: runs the gate on every push and pull request. On a `v*` tag it builds `cpu` and `vulkan` binaries on Ubuntu 24.04 with `GGML_NATIVE=OFF` (an AVX2 baseline instead of the build machine's CPU), packages them with `scripts/package.sh`, and publishes a GitHub release with `SHA256SUMS`.
- `scripts/install.sh`: installs dependencies, the release binary for the machine (Vulkan when a GPU is present), the speech model, and the systemd user service. `--speak` adds the Kokoro voice; `--tts-cuda` builds from source with ONNX Runtime's CUDA provider, whose two libraries go next to the binary in `~/.local/bin`: ONNX Runtime loads them from the executable's own directory by full path, so neither RUNPATH nor `LD_LIBRARY_PATH` would find them elsewhere.
