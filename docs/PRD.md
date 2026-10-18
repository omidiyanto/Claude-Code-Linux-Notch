# PRD: Claude Notch, a voice companion for Claude Code on the Linux desktop

| | |
|---|---|
| Status | Shipped: v0.3 |
| Platforms | Linux: KDE Plasma 6 (Wayland), Hyprland, Sway, COSMIC; GNOME through XWayland |
| Design | [ARCHITECTURE.md](ARCHITECTURE.md) |

## 1. Summary

Claude Notch is a small "dynamic island" at the top of the screen with an animated, Claude-orange
pixel buddy. It runs all the time and listens to the microphone. When the user says
**"Hi Claude"**, the notch reacts, transcribes the command on the computer, and hands it to
**Claude Code** (`claude -p`), which carries it out on the machine: opening apps, controlling the
browser, running commands, starting a new coding session.

The goal is to talk to Claude Code as an agentic companion, not only type to it in a terminal.

## 2. Problem and goals

**Problem:** Claude Code is only reachable by typing in a terminal. Small everyday tasks ("open
YouTube", "start a new project") still mean switching to a terminal and typing.

**Goals**
1. One spoken sentence becomes a real action on the desktop, without touching the keyboard.
2. A lively, likeable presence: a character that reacts, not a static icon.
3. Safe: a voice command can never do something destructive that the user did not ask for.
4. Private: audio never leaves the computer; only the text of a command goes to Claude.
5. One source of truth: Claude behaves the same way it does in the terminal, configured in Claude
   Code's own settings.

**Non-goals:** a phone app, cloud sync, dictation into other apps, multiple users, replacing the
terminal UI of Claude Code.

## 3. Persona and scenarios

**Persona:** a Linux developer who uses Claude Code daily and speaks English, Indonesian, or both.

| # | The user says / does | What happens |
|---|---|---|
| S1 | "Hi Claude" | The notch grows, the buddy perks up, a chime plays, and live captions show what is being said. |
| S2 | "Hi Claude, open YouTube and play my mix" | Transcript → *Thinking* → Claude Code drives the browser (Claude in Chrome) or opens the URL → the notch shows a one-line answer. |
| S3 | "Hi Claude, let's code, start a new Rust project called todo-api" | Claude creates `~/Projects/todo-api` and opens a new terminal with an interactive `claude` session. |
| S4 | A risky action Claude was not asked for | Auto mode's classifier blocks it; Claude says what was blocked. Confirming it explicitly in a follow-up lets it through. |
| S5 | Within 60 s of an answer: "pause it" | A follow-up without the wake phrase. Every command continues the same conversation until the notch restarts. |
| S6 | Silence | The buddy breathes, blinks and glances around; after 2 minutes it falls asleep and uses almost no CPU. |
| S7 | Cannot speak right now | Click the notch (push-to-talk), use a keyboard shortcut (`claude-notch listen`), or type (`claude-notch ask "…"`). |
| S8 | Clicks the ⚙ on the notch | The settings window: permission mode, model, effort, connectivity test, personality, history, voice. |
| S9 | Drags the notch | It moves anywhere; near the top edge it snaps back into a notch, elsewhere it floats as an island. |

## 4. Functional requirements

| ID | Requirement | Status |
|---|---|---|
| F1 | A notch above every window, centred at the top, clicks outside the pill pass through, smooth spring animations between sizes | ✅ |
| F2 | A procedurally drawn pixel character with states: idle, listening, transcribing, thinking, working, approval, done, error, sleep, paused | ✅ |
| F3 | Always-on wake phrase ("hi/hey/hai/halo Claude", configurable), local speech recognition, English and Indonesian, live captions, follow-up window | ✅ |
| F4 | Each command runs Claude Code headless in a private voice workspace; one conversation per run of the notch | ✅ |
| F5 | Claude's model, effort, permission mode and rules come from Claude Code's own settings files (user scope, plus a voice-only project scope) | ✅ |
| F6 | Permission prompts that Claude Code raises appear on the notch as Allow / Deny | ✅ |
| F7 | Browser control through Claude in Chrome when the user turned it on with `/chrome` | ✅ |
| F8 | A settings window with a connectivity test, personality editor (`CLAUDE.md`) and voice history (Claude Code's transcripts) | ✅ |
| F9 | Draggable notch, remembered position | ✅ |
| F10 | systemd user service, restart from the settings window, single instance | ✅ |
| F11 | Spoken answers (text-to-speech) | Not planned (text in the notch plus sound effects, by choice) |

## 5. Non-functional requirements (measured)

| Aspect | Target | Measured |
|---|---|---|
| End of speech → text, 3 s command | < 3 s | 0.46 s (large-v3-turbo, Vulkan, Quadro T1000) · 1.3 s (small-q5_1, CPU) |
| Idle CPU (listening, nobody speaking) | low | ~1.8 % of one core for the app + ~1.8 % for WebKit |
| Animation | smooth | 30 fps when active, 12 fps resting, 4 fps asleep |
| Privacy | audio stays local | No audio is ever written to disk or sent; transcripts are logged only with `CLAUDE_NOTCH_DEBUG=1` |
| Robustness | never breaks Claude Code | Claude Code's settings are written only on Save, key by key, with a backup; a broken file is never touched |
| Memory | — | ~850 MB RSS with a speech model loaded (about 300 MB of it shared WebKit/GTK libraries) |

## 6. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Misheard wake phrase or language | Wake phrase variants ("cloud", "klod"); language restricted to the user's languages; recommended GPU model |
| Background music or TV | Only the first 3 s of an utterance are checked for the wake phrase; follow-ups longer than 12 s are ignored; auto mode's classifier stops actions that were not asked for |
| Apps opened by voice dying with the service | `KillMode=process` plus the notch's own SIGTERM handling |
| Claude Code CLI changes | All CLI use in one module (`brain.rs`), with fixtures of real output |
| Brand | The character, icon and sounds are original and drawn in code; Claude and Claude Code are Anthropic trademarks and this is not an Anthropic product |
