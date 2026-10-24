# Buddy - Your Realtime Voice AI Assistant

[![CI](https://github.com/omidiyanto/Claude-Code-Linux-Notch/actions/workflows/ci.yaml/badge.svg)](https://github.com/omidiyanto/Claude-Code-Linux-Notch/actions/workflows/ci.yaml)

A dynamic island at the top of your Linux desktop, with an orange pixel buddy that hands your voice to **Claude Code**.

Say **"Hi Claude"**, then a command such as *"open YouTube and play my mix"* or *"let's code, start a new Rust project called todo-api"*. The notch listens, Claude Code acts on your computer, and the answer appears in the notch.

- Speech recognition runs **on your computer** (whisper.cpp). Audio is never saved and never sent anywhere; only the text of the command goes to Claude Code.
- The brain is your own **Claude Code** (`claude -p`), with the login you already have. No extra API key.
- **One source of truth:** model, effort and permission mode come from **Claude Code's own settings files**, the same ones `claude` in your terminal uses. Voice-only overrides go in Claude Code's project scope for the voice workspace.
- **Browser control** through Claude in Chrome, when you turned it on with `/chrome`.
- **Settings window** (the ⚙ on the notch): permission mode, model, effort, a connectivity test, personality, history, and the notch itself. Drag the notch anywhere; away from the top edge it becomes a floating island.

Docs: [product requirements](docs/PRD.md) · [architecture and measured decisions](docs/ARCHITECTURE.md)

## Quick start

One command installs everything (system libraries, the prebuilt binary, a speech model, and a systemd user service that starts with your desktop):

```bash
curl -fsSL https://raw.githubusercontent.com/omidiyanto/Claude-Code-Linux-Notch/main/scripts/install.sh | bash
```

It picks the **GPU (Vulkan) build with the large-v3-turbo model** when it finds a dedicated GPU, and the **CPU build with small-q5_1** otherwise. Downloads are checked against the release's `SHA256SUMS`. Run it again to update.

```bash
# options go after "bash -s --"
curl -fsSL https://raw.githubusercontent.com/omidiyanto/Claude-Code-Linux-Notch/main/scripts/install.sh | bash -s -- --cpu
#   --cpu | --gpu          force a build          --model NAME     speech model
#   --version vX.Y.Z       a specific release     --no-service     no systemd service
#   --from-source          build this checkout    --uninstall [--purge]
```

You also need [Claude Code](https://code.claude.com/docs) installed and signed in (`claude`, then `/login`).

**Requirements:** Linux x86_64 with glibc 2.39 or newer for the prebuilt binaries (Arch, Fedora 40+, Ubuntu 24.04+, Debian 13+; older systems: `--from-source`); a CPU with AVX2; a Wayland compositor with wlr-layer-shell (**KDE Plasma 6, Hyprland, Sway, COSMIC**) or GNOME (through XWayland); a microphone.

**Push-to-talk shortcut (recommended):** in KDE go to *System Settings → Keyboard → Shortcuts → Add New → Command or Script*, enter `claude-notch listen`, and choose a key such as `Meta+Space`.

## Running it

The installer sets up a systemd user service that starts with your desktop session, restarts after a crash, and logs to the journal:

```bash
systemctl --user status claude-notch         # is it running?
journalctl --user -u claude-notch -f         # its log
systemctl --user restart claude-notch        # restart it
systemctl --user disable --now claude-notch  # stop it and don't start on login
```

Stopping or restarting the service also stops a command Claude is running at that moment. Apps the notch opened (a browser, a "let's code" terminal) keep running (`KillMode=process`). They do show up under the service in `systemctl --user status`.

## Speech recognition: which model

| Model | Size | Quality | 3 s command, measured |
|---|---|---|---|
| `small-q5_1` (CPU default) | 190 MB | OK for clear English | ~1.3 s on CPU |
| `large-v3-turbo-q5_0` (**recommended**, GPU default) | 574 MB | Much better, especially with accents and background noise | ~0.46 s on a GPU (Quadro T1000) · ~6.7 s on CPU |

Switch models in Settings → Voice & notch, after downloading one with `download-model.sh` (in the release archive and in `scripts/`).

Speech is recognized as English (`language = "en"`). With `language = "auto"`, each utterance is recognized in whichever of `auto_languages` Whisper finds most likely, never another language: left free, Whisper sometimes heard accented English as another language and wrote a translation instead of what was said.

## Reading answers aloud (Kokoro)

Answers can be spoken by [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M), running on this computer, in English. Turn it on with **Read answers aloud** in Settings → Voice & notch, after:

```bash
sudo pacman -S --needed espeak-ng          # phonemes (Debian/Ubuntu: espeak-ng)
scripts/download-model.sh kokoro           # the model and two voices, ~330 MB, checksums pinned
```

- On the CPU it reads about 4× faster than real time. On an NVIDIA GPU, build with `--features tts-cuda` (below): ONNX Runtime's CUDA provider, which needs CUDA 13 and cuDNN 9 installed (`sudo pacman -S cuda cudnn`).
- The microphone is off while it speaks, so the notch never hears itself. Click the notch, press ✕ or give a new command to stop it. The follow-up window opens when it has finished.
- Voices: `voice = "af_heart"` (default) or `"am_michael"`; any other Kokoro voice file put in `~/.local/share/claude-notch/models/kokoro/voices/` works too.
- Sound goes out through `pw-play` (PipeWire).

## Building from source

```bash
# Arch (Debian/Ubuntu: see BUILD_PACKAGES in .github/workflows/ci.yaml)
sudo pacman -S --needed webkit2gtk-4.1 gtk-layer-shell alsa-lib cmake clang
sudo pacman -S --needed vulkan-headers shaderc vulkan-icd-loader   # for the GPU build

cargo build --release                      # CPU
cargo build --release --features vulkan    # GPU: NVIDIA, AMD and Intel through Vulkan
cargo build --release --features cuda      # GPU: NVIDIA only, needs CUDA
cargo build --release --features vulkan,tts-cuda   # plus spoken answers on an NVIDIA GPU (CUDA 13, cuDNN 9)

scripts/install.sh --from-source           # or: build, install and start it in one go
```

Rust 1.92 or newer is needed.

## Usage

| What you do | What happens |
|---|---|
| Say "Hi Claude" | The buddy wakes up and listens. Then say your command, and the words appear in the notch as you speak (live captions). |
| Say "Hey Claude, open YouTube" | The wake phrase and the command in one sentence. |
| Speak within 60 seconds of an answer | A follow-up without the wake phrase (an orange dot shows this window). |
| Any later command | Continues the same Claude conversation: context is kept until Claude Notch restarts or stops. If that conversation can no longer be loaded (its history was deleted), the command runs in a new one. |
| Say "Hey Claude, stop" / "cancel" | Cancels what Claude is doing. |
| Hover the top edge where the notch sits | It slides out. At rest it stays tucked away, and comes out by itself whenever something happens (listening, thinking, an answer). |
| Click the notch | Push-to-talk. |
| Drag the notch | Moves it anywhere. Drop it near the top edge and it snaps back into place as a notch. |
| ⚙ (hover the notch, or while it is open) | Opens the settings window. |
| Right-click the notch | Turns the microphone off or on. The device really is closed, and the buddy turns grey. |
| ✕ | Cancel. |
| Something gets blocked | Claude says what was blocked. Confirm it in a follow-up by naming the action and its target exactly ("yes, delete the folder ~/Downloads/tmp"). |
| An approval card appears | Only when Claude Code asks: in the "Ask every action" mode, or when auto mode pauses after repeated blocks. **Allow** or **Deny**. |

CLI (talks to the running notch):

```bash
claude-notch listen              # start listening
claude-notch ask "open youtube"  # typed command, no voice needed
claude-notch cancel
claude-notch pause
claude-notch settings            # open the settings window
claude-notch demo approval       # preview a look: idle, sleep, listening, thinking, working, approval, done, error
```

Recognised wake phrases: *hi / hey / hello / ok / okay* + *Claude*, including common recognition slips ("cloud", "klod").

## Settings: where each one lives

Everything can be changed in the settings window. Each setting is stored in the file that owns it:

| Setting | File | Scope |
|---|---|---|
| Model, effort, permission mode, `permissions.allow` | `~/.claude/settings.json` | **All Claude Code sessions** (Claude Code's user scope) |
| Same keys, voice only | `~/.local/share/claude-notch/workspace/.claude/settings.json` | **Voice commands only** (Claude Code's project scope; overrides the user scope) |
| Personality, preferences, shortcuts | `~/.local/share/claude-notch/workspace/CLAUDE.md` | Voice commands |
| Wake phrases, speech recognition, microphone, follow-up, sounds, notch position | `~/.config/claude-notch/config.toml` | Claude Notch only |
| History | Claude Code's transcripts, `~/.claude/projects/-home-…-claude-notch-workspace/` | Voice sessions |

The voice scope starts with `effortLevel: "low"` and a small allowlist of safe commands. Claude Code itself enforces two rules: **auto mode** and `bypassPermissions` take effect only from the user scope, never from a project file such as the voice scope.

How Claude Notch writes Claude Code's settings files, so it never breaks them:
- Only when you press **Save** in the Claude tab, and only the keys whose value you changed. Every other key keeps its value and its place in the file.
- The file is re-read just before it is replaced, in case Claude Code (`/config`, `/model`) wrote it meanwhile. The write is atomic and keeps the file's permissions.
- A backup of the previous version goes to `~/.local/share/claude-notch/backups/` (the last 10 per file), never into `~/.claude`.
- A file that is not valid JSON is shown as an error and never touched. `bypassPermissions` is never written.
- If your user settings ever set `bypassPermissions`, voice commands run in `auto` instead.

`config.toml` (every key optional; the defaults are shown):

```toml
wake_phrases = ["hi claude", "hey claude", "hello claude", "ok claude", "okay claude"]
language = "en"              # a Whisper language code, or "auto": per utterance, the likeliest of auto_languages
auto_languages = ["en"]
# model_path = "~/.local/share/claude-notch/models/ggml-small-q5_1.bin"
claude_bin = "claude"
follow_up_secs = 60          # 0 turns follow-up off
turn_timeout_secs = 300
sounds = true
speak = false                # read answers aloud with Kokoro (see above)
voice = "af_heart"
vad_threshold = 0.012        # raise it if the room is noisy
position = { x = 0.5, y = 0.0 }  # centre as a fraction of the screen width, pixels from the top
```

Wake phrases, language, speech model, microphone sensitivity and `claude_bin` take effect after a restart (the settings window offers one). The rest applies at once.

Never add generic launchers such as `Bash(setsid:*)`, `Bash(konsole:*)` or `Bash(bash:*)` to an allowlist: each of them would let any command through without review.

## How it works

```
mic → cpal 16 kHz → VAD (energy) → whisper.cpp → wake phrase? → state machine → claude -p (stream-json)
                                                                    ↕                   ↓ permission prompt
                                                     notch (Tauri + layer-shell) ← claude-notch mcp-approve
```

| File | Role |
|---|---|
| `src/machine.rs` | Pure state machine (events → effects); all behaviour is unit-tested here |
| `src/wake.rs`, `src/vad.rs` | Wake phrase and speech detection (pure, tested) |
| `src/brain.rs` | Runs `claude -p` in the voice workspace: prompt through stdin, stream-json parsing, cancellation per process group, connectivity test |
| `src/claude_settings.rs` | Reads and merges Claude Code's settings files (user and voice scope) |
| `src/settings.rs`, `src/history.rs` | Settings window commands; history from Claude Code's transcripts |
| `src/approve.rs` | MCP server for `--permission-prompt-tool`: forwards each request to the notch |
| `src/ipc.rs` | Unix socket `$XDG_RUNTIME_DIR/claude-notch.sock` (0600 + peer UID check) |
| `src/island.rs` | Layer-shell overlay + input region (clicks outside the pill pass through) |
| `src/audio.rs`, `src/stt.rs` | Microphone and whisper.cpp |
| `ui/` | Notch page (`index.html`): pixel buddy (`sprite.js`), sound effects (`sfx.js`), dragging (`position.js`). Settings window (`settings.html`) |

## Security and privacy

- No `--model`, `--effort`, `--permission-mode` or `--allowedTools` flags: Claude Code's own settings decide, with its own safety rules. The one exception: an inherited `bypassPermissions` is lowered to `auto`, because a TV or someone else in the room can start a voice command.
- The prompt goes through **stdin**, never argv: a transcript can never be read as a CLI flag. The `--resume` session id is checked to be a UUID.
- Approval requests that do not come from a command you gave are denied automatically. When a run ends, is cancelled or times out, every pending request is denied.
- The page only ever renders text (`textContent`, lint-enforced), under a strict CSP, with the minimum Tauri capabilities.
- Transcripts are not logged unless `CLAUDE_NOTCH_DEBUG=1` is set.

## Development

```bash
scripts/check.sh          # rustfmt, clippy (pedantic, -D warnings), Rust tests, cargo audit,
                          # npm ci, eslint, UI tests, npm audit, shellcheck
scripts/check.sh --slow   # plus end-to-end speech tests (espeak-ng → whisper → wake phrase)
CLAUDE_NOTCH_DEBUG=1 cargo run
```

Environment variables: `CLAUDE_NOTCH_DEBUG=1` (log transcripts), `CLAUDE_NOTCH_LAYER_SHELL=0` (force a normal window).

## Known limitations

- Wake phrase detection runs Whisper on the first 3 s of every stretch of speech: about 1 s of CPU work per utterance (small-q5_1, 6 cores). TV or music in the background can wake it; in auto mode the classifier, not a click, is then what stops an unwanted action.
- Live captions appear after "Hi Claude" + a pause (or a click). "Hi Claude, open YouTube" said in one breath has no captions: the text appears once the sentence is finished.
- Captions are refreshed about every 1.5 s on CPU (each refresh re-transcribes the sentence so far). A GPU build (`--features cuda`) makes them smoother.
- Claude Code's own `/voice` is not used: it only works inside the interactive terminal UI (hold or tap Space), with no CLI flag or API for other programs. It also sends audio to Anthropic's servers. Inside a "let's code" session that the notch opens, you can still use `/voice` yourself.
- Only one monitor: the notch appears where the compositor puts it (usually the primary or focused monitor). Dragging moves it within that monitor.
- Approvals offer Allow/Deny only, with no "Always allow"; add permanent rules to `permissions.allow` in the Claude tab instead.

## Credits

The layer-shell and input-region approach is adapted from [Coucou](https://github.com/Louis-CFM/coucou) by Louis Raillé (MIT). No Coucou/Mochi assets are used: the character, sounds and icon here are generated in code. Claude and Claude Code are trademarks of Anthropic; this is a personal project, not affiliated with Anthropic.
