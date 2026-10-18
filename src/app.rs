//! Wires everything together: microphone → recognition → state machine → Claude Code, and the
//! state machine → the page. Every decision is made in machine.rs; this file carries them out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tokio::sync::{mpsc, oneshot};

use crate::audio::{self, AudioEvent};
use crate::brain::{self, BrainEvent};
use crate::config::{self, Config};
use crate::ipc::{self, BindError, Incoming};
use crate::island;
use crate::machine::{Effect, Event, Machine, Mode, Ui};
use crate::settings;
use crate::stt::{self, Need, SttEvent};

/// Exit status that makes the systemd unit start the notch again (`RestartForceExitStatus=75`).
const RESTART_STATUS: i32 = 75;

#[derive(Debug)]
pub enum Input {
    Event(Event),
    /// An event of the Claude run with this generation; older runs' events are dropped.
    Brain(u64, BrainEvent),
    Ipc(Incoming),
    /// SIGTERM or SIGINT: stop the running command, then quit.
    Quit,
    /// Saved notch settings; the ones read only at startup wait for a restart.
    Config(Arc<Config>),
    /// Stop, then start again with the saved settings.
    Restart,
}

pub struct Shared {
    pub tx: mpsc::UnboundedSender<Input>,
    pub ui: Arc<Mutex<Ui>>,
    /// The notch settings as saved; written under this lock only.
    pub config: Mutex<Config>,
    /// The settings this process started with.
    pub started: Config,
    pub workspace: PathBuf,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Action {
    Listen,
    Cancel,
    Pause,
    Allow,
    Deny,
}

#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri passes command arguments by value")]
fn current_ui(shared: tauri::State<'_, Shared>) -> Ui {
    shared.ui.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri passes command arguments by value")]
fn ui_action(shared: tauri::State<'_, Shared>, action: Action, id: Option<u64>) -> Result<(), String> {
    let event = match (action, id) {
        (Action::Listen, _) => Event::ListenRequested,
        (Action::Cancel, _) => Event::CancelRequested,
        (Action::Pause, _) => Event::PauseToggled,
        (Action::Allow, Some(id)) => Event::ApprovalAnswered { id, allow: true },
        (Action::Deny, Some(id)) => Event::ApprovalAnswered { id, allow: false },
        (Action::Allow | Action::Deny, None) => return Err("an approval id is required".into()),
    };
    shared.tx.send(Input::Event(event)).map_err(|_| "Claude Notch is shutting down".into())
}

#[tauri::command]
#[expect(clippy::needless_pass_by_value, reason = "Tauri passes command arguments by value")]
fn set_hit_region(window: WebviewWindow, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
    let rect = island::clamp_region(x, y, width, height).ok_or("invalid region")?;
    island::set_region(&window, rect)
}

struct Driver {
    cfg: Arc<Config>,
    app: AppHandle,
    tx: mpsc::UnboundedSender<Input>,
    ui: Arc<Mutex<Ui>>,
    mic: Option<std::sync::mpsc::Sender<bool>>,
    want_level: Arc<AtomicBool>,
    need: Arc<AtomicU8>,
    workspace: PathBuf,
    exe: PathBuf,
    generation: u64,
    cancel: Option<oneshot::Sender<()>>,
    brain: Option<tauri::async_runtime::JoinHandle<()>>,
    waiting: HashMap<u64, oneshot::Sender<bool>>,
    next_id: u64,
}

impl Driver {
    async fn run(mut self, mut machine: Machine, mut rx: mpsc::UnboundedReceiver<Input>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let restart = loop {
            let effects = tokio::select! {
                input = rx.recv() => match input {
                    Some(Input::Quit) | None => break false,
                    Some(Input::Restart) => break true,
                    Some(Input::Config(cfg)) => {
                        machine.set_follow_up(Duration::from_secs(cfg.follow_up_secs));
                        self.cfg = cfg;
                        Vec::new()
                    }
                    Some(input) => self.input(&mut machine, input),
                },
                _ = tick.tick() => {
                    let now = Instant::now();
                    // A permission server that gave up (timeout, claude gone) withdraws its question.
                    let gone: Vec<u64> = self.waiting.iter().filter(|(_, r)| r.is_closed()).map(|(id, _)| *id).collect();
                    let mut fx = Vec::new();
                    for id in gone {
                        self.waiting.remove(&id);
                        fx.extend(machine.handle(Event::ApprovalAnswered { id, allow: false }, now));
                    }
                    fx.extend(machine.handle(Event::Tick, now));
                    fx
                }
            };
            for effect in effects {
                self.apply(effect);
            }
        };
        self.stop().await;
        self.app.exit(if restart { start_again() } else { 0 });
    }

    /// Stops a running command and everything in its process group. Apps it opened with
    /// `setsid -f` are in their own session and live on. Unanswered approvals are dropped,
    /// which the permission server reads as a denial.
    async fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(brain) = self.brain.take() {
            // brain::run gives claude 2 s after SIGTERM before SIGKILL.
            let _ = tokio::time::timeout(Duration::from_secs(3), brain).await;
        }
        self.waiting.clear();
    }

    fn input(&mut self, machine: &mut Machine, input: Input) -> Vec<Effect> {
        let now = Instant::now();
        match input {
            Input::Event(event) => machine.handle(event, now),
            Input::Brain(generation, event) if generation == self.generation => {
                machine.handle(Event::Brain(event), now)
            }
            Input::Brain(..) | Input::Quit | Input::Restart | Input::Config(_) => Vec::new(),
            Input::Ipc(Incoming::Listen) => machine.handle(Event::ListenRequested, now),
            Input::Ipc(Incoming::Cancel) => machine.handle(Event::CancelRequested, now),
            Input::Ipc(Incoming::Pause) => machine.handle(Event::PauseToggled, now),
            Input::Ipc(Incoming::Ask(text)) => machine.handle(Event::Typed(text), now),
            Input::Ipc(Incoming::Settings(tab)) => {
                if let Err(e) = settings::open_window(&self.app, tab) {
                    crate::log(&format!("settings: {e}"));
                }
                Vec::new()
            }
            Input::Ipc(Incoming::Demo(mode)) => {
                self.demo(mode);
                Vec::new()
            }
            Input::Ipc(Incoming::Approve { tool_name, input, reply }) => {
                self.next_id += 1;
                self.waiting.insert(self.next_id, reply);
                let detail = brain::approval_detail(&tool_name, &input);
                machine.handle(Event::ApprovalRequested { id: self.next_id, tool: tool_name, detail }, now)
            }
        }
    }

    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::Ui(ui) => {
                let listening = matches!(ui.mode, Mode::Listening | Mode::Transcribing);
                self.want_level.store(listening, Ordering::Relaxed);
                let need = match ui.mode {
                    _ if listening => Need::Everything,
                    _ if ui.follow_up => Need::FollowUp,
                    _ => Need::Wake,
                };
                self.need.store(need as u8, Ordering::Relaxed);
                *self.ui.lock().unwrap_or_else(PoisonError::into_inner) = ui.clone();
                let _ = self.app.emit("notch", ui);
            }
            Effect::Sfx(sfx) => {
                if self.cfg.sounds {
                    let _ = self.app.emit("sfx", sfx);
                }
            }
            Effect::StartBrain { prompt, resume } => {
                self.generation += 1;
                let (cancel, cancelled) = oneshot::channel();
                self.cancel = Some(cancel);
                let (generation, tx, cfg) = (self.generation, self.tx.clone(), self.cfg.clone());
                let (workspace, exe) = (self.workspace.clone(), self.exe.clone());
                self.brain = Some(tauri::async_runtime::spawn(async move {
                    let emit = |event| {
                        let _ = tx.send(Input::Brain(generation, event));
                    };
                    brain::run(&cfg, &prompt, resume.as_deref(), &workspace, &exe, cancelled, emit).await;
                }));
            }
            Effect::CancelBrain => {
                if let Some(cancel) = self.cancel.take() {
                    let _ = cancel.send(());
                }
                self.generation += 1;
            }
            Effect::Approve { id, allow } => {
                if let Some(reply) = self.waiting.remove(&id) {
                    let _ = reply.send(allow);
                }
            }
            Effect::Mic(on) => {
                if let Some(mic) = &self.mic {
                    let _ = mic.send(on);
                }
            }
        }
    }

    /// `claude-notch demo <mode>`: shows one look without touching the state machine.
    fn demo(&self, mode: Mode) {
        let (title, detail) = match mode {
            Mode::Thinking | Mode::Working => ("open YouTube and play my mix", "$ xdg-open https://youtube.com"),
            Mode::Approval => ("Bash", "mkdir -p ~/Projects/todo-api && setsid -f konsole -e claude"),
            Mode::Done => ("open YouTube", "YouTube is open in your browser. Enjoy!"),
            Mode::Error => ("open YouTube", "Claude took longer than 300 s and was stopped"),
            _ => ("", ""),
        };
        let ui = Ui {
            mode,
            title: title.into(),
            detail: detail.into(),
            approval: (mode == Mode::Approval).then_some(0),
            paused: false,
            follow_up: false,
        };
        let _ = self.app.emit("notch", ui);
    }
}

/// Arranges for a new notch to start once this one has exited; returns this one's exit status.
/// Under the systemd unit, systemd does it; otherwise a new process waits for this one to go.
fn start_again() -> i32 {
    let in_unit = std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|c| c.contains("/claude-notch.service"));
    if in_unit {
        return RESTART_STATUS;
    }
    let spawned = std::env::current_exe().and_then(|exe| {
        let mut command = std::process::Command::new(exe);
        command.env(crate::REPLACES_VAR, std::process::id().to_string()).stdin(std::process::Stdio::null());
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command.spawn()
    });
    if let Err(e) = spawned {
        crate::log(&format!("cannot start again: {e}"));
    }
    0
}

/// Waits (up to 15 s) for the notch this one replaces to exit and free the socket.
fn wait_for_exit(pid: u32) {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    for _ in 0..150 {
        if !proc.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// What the app needs once Tauri has made its window.
struct Startup {
    cfg: Arc<Config>,
    listener: std::os::unix::net::UnixListener,
    workspace: PathBuf,
    exe: PathBuf,
    machine: Machine,
    ui: Arc<Mutex<Ui>>,
    tx: mpsc::UnboundedSender<Input>,
    rx: mpsc::UnboundedReceiver<Input>,
}

impl Startup {
    fn start(self, app: &tauri::App) -> Result<(), String> {
        let Self { cfg, listener, workspace, exe, machine, ui, tx, rx } = self;
        let win = app.get_webview_window("island").ok_or("the island window is missing")?;
        island::setup(&win)?;
        let handle = app.handle().clone();
        let want_level = Arc::new(AtomicBool::new(false));
        let need = Arc::new(AtomicU8::new(Need::Wake as u8));
        let mic = start_voice(&cfg, &handle, &tx, &want_level, &need);

        let (ipc_tx, mut ipc_rx) = mpsc::unbounded_channel();
        tauri::async_runtime::spawn(ipc::serve(listener, ipc_tx));
        let forward = tx.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(incoming) = ipc_rx.recv().await {
                if forward.send(Input::Ipc(incoming)).is_err() {
                    break;
                }
            }
        });
        watch_signals(tx.clone());

        let driver = Driver {
            cfg,
            app: handle,
            tx,
            ui,
            mic,
            want_level,
            need,
            workspace,
            exe,
            generation: 0,
            cancel: None,
            brain: None,
            waiting: HashMap::new(),
            next_id: 0,
        };
        tauri::async_runtime::spawn(driver.run(machine, rx));
        Ok(())
    }
}

/// Without this, systemd stopping the service or Ctrl+C would orphan a running `claude -p`:
/// it is in its own process group, so the signal never reaches it.
fn watch_signals(quit: mpsc::UnboundedSender<Input>) {
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
            crate::log("cannot watch SIGTERM/SIGINT; a running command would outlive the notch");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        crate::log("stopping");
        let _ = quit.send(Input::Quit);
    });
}

/// Starts recognition and the microphone; a problem is shown on the notch instead.
fn start_voice(
    cfg: &Config,
    app: &AppHandle,
    tx: &mpsc::UnboundedSender<Input>,
    want_level: &Arc<AtomicBool>,
    need: &Arc<AtomicU8>,
) -> Option<std::sync::mpsc::Sender<bool>> {
    let problem = |message: String| {
        let _ = tx.send(Input::Event(Event::Problem(message)));
        None
    };
    let model = cfg.model_path();
    if !model.is_file() {
        return problem(format!("Speech model missing: run scripts/download-model.sh ({})", model.display()));
    }
    let to_machine = tx.clone();
    let debug = std::env::var_os("CLAUDE_NOTCH_DEBUG").is_some();
    let stt = stt::spawn(model, cfg.speech_languages(), cfg.wake_phrases.clone(), need.clone(), move |event| {
        let event = match event {
            SttEvent::Started => Event::Transcribing,
            SttEvent::Failed(e) => Event::Problem(e),
            SttEvent::Partial(text) => Event::Partial(text),
            SttEvent::Text(text) => {
                // Transcripts stay private unless the user asks for them in the log.
                if debug {
                    crate::log(&format!("heard: {text:?}"));
                }
                Event::Transcript(text)
            }
        };
        let _ = to_machine.send(Input::Event(event));
    });
    let (to_machine, app, want_level, need) = (tx.clone(), app.clone(), want_level.clone(), need.clone());
    let mic = audio::spawn(cfg.vad_threshold, move |event| match event {
        AudioEvent::Level(level) => {
            if want_level.load(Ordering::Relaxed) {
                let _ = app.emit("level", level);
            }
        }
        AudioEvent::Speech => {
            let _ = to_machine.send(Input::Event(Event::SpeechStarted));
        }
        // Live captions only while the user is giving a command; never for room chatter.
        AudioEvent::Partial(samples) => {
            if Need::from_u8(need.load(Ordering::Relaxed)) == Need::Everything {
                stt.partial(samples);
            }
        }
        AudioEvent::Utterance(samples) => {
            if !stt.utterance(samples) {
                crate::log("speech recognition is busy; an utterance was skipped");
            }
        }
    });
    match mic {
        Ok(mic) => Some(mic),
        Err(e) => problem(e),
    }
}

pub fn run(replaces: Option<u32>) -> ExitCode {
    let fail = |message: String| {
        crate::log(&message);
        ExitCode::FAILURE
    };
    if let Some(pid) = replaces {
        wait_for_exit(pid);
    }
    let cfg = match Config::load(&config::config_file()) {
        Ok(cfg) => Arc::new(cfg),
        Err(e) => return fail(format!("config: {e}")),
    };
    let socket = match config::socket_path() {
        Ok(path) => path,
        Err(e) => return fail(e),
    };
    let listener = match ipc::bind(&socket) {
        Ok(listener) => listener,
        Err(BindError::AlreadyRunning) => {
            crate::log("already running");
            return ExitCode::SUCCESS;
        }
        Err(BindError::Io(e)) => return fail(e),
    };
    let workspace = config::workspace_dir();
    if let Err(e) = config::ensure_workspace(&workspace) {
        return fail(e);
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return fail(format!("own path: {e}")),
    };

    let machine = Machine::new(cfg.wake_phrases.clone(), Duration::from_secs(cfg.follow_up_secs), Instant::now());
    let ui = Arc::new(Mutex::new(machine.ui(Instant::now())));
    let (tx, rx) = mpsc::unbounded_channel();
    let shared = Shared {
        tx: tx.clone(),
        ui: ui.clone(),
        config: Mutex::new((*cfg).clone()),
        started: (*cfg).clone(),
        workspace: workspace.clone(),
    };
    let startup = Startup { cfg, listener, workspace, exe, machine, ui, tx, rx };

    let result = tauri::Builder::default()
        .manage(shared)
        .invoke_handler(tauri::generate_handler![
            current_ui,
            ui_action,
            set_hit_region,
            settings::open_settings,
            settings::get_settings,
            settings::save_notch,
            settings::save_claude,
            settings::read_personality,
            settings::save_personality,
            settings::test_claude,
            settings::list_history,
            settings::delete_history,
            settings::resume_session,
            settings::open_location,
            settings::restart_notch,
            settings::notch_position,
            settings::save_position,
            settings::reset_position,
        ])
        .setup(move |app| startup.start(app).map_err(Into::into))
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(format!("tauri: {e}")),
    }
}
