//! The notch's state machine: events in, effects out. No I/O and no clock of its own
//! (the time comes with every event), so every behaviour below is a unit test.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::brain::{BrainEvent, truncate};
use crate::wake::{self, Heard};

/// Waiting for a command after "hi Claude" or a click.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(8);
/// Long enough for a 30 s utterance plus recognition.
const UTTERANCE_TIMEOUT: Duration = Duration::from_secs(45);
const SLEEP_AFTER: Duration = Duration::from_secs(120);
const ERROR_SHOWN: Duration = Duration::from_secs(8);
const MAX_TITLE: usize = 120;
const MAX_DETAIL: usize = 400;
/// A live caption shows the newest words, not the first ones.
const MAX_CAPTION: usize = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Sleep,
    Idle,
    Listening,
    Transcribing,
    Thinking,
    Working,
    Approval,
    Done,
    Error,
}

/// Everything the page draws; it holds no state of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ui {
    pub mode: Mode,
    pub title: String,
    pub detail: String,
    pub approval: Option<u64>,
    pub paused: bool,
    pub follow_up: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sfx {
    Wake,
    Done,
    Error,
    Attention,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    SpeechStarted,
    Transcribing,
    Transcript(String),
    /// A live caption of what is being said right now.
    Partial(String),
    /// A typed command: no wake phrase, works with the microphone paused.
    Typed(String),
    ListenRequested,
    CancelRequested,
    PauseToggled,
    Brain(BrainEvent),
    ApprovalRequested {
        id: u64,
        tool: String,
        detail: String,
    },
    ApprovalAnswered {
        id: u64,
        allow: bool,
    },
    Problem(String),
    Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Ui(Ui),
    Sfx(Sfx),
    StartBrain { prompt: String, resume: Option<String> },
    CancelBrain,
    Approve { id: u64, allow: bool },
    Mic(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Sleep,
    /// `heard` is the live caption of the command being spoken.
    Listening {
        until: Instant,
        transcribing: bool,
        heard: String,
    },
    /// `resumed`: this run continues the conversation; `started`: Claude Code reported its
    /// session, so the run got past loading it.
    Busy {
        prompt: String,
        tool: Option<String>,
        text: String,
        resumed: bool,
        started: bool,
    },
    Shown {
        error: bool,
        title: String,
        detail: String,
        until: Instant,
    },
}

#[derive(Debug)]
struct Approval {
    id: u64,
    tool: String,
    detail: String,
}

#[derive(Debug)]
pub struct Machine {
    phrases: Vec<String>,
    follow_up: Duration,
    state: State,
    paused: bool,
    approvals: VecDeque<Approval>,
    session: Option<String>,
    follow_up_until: Option<Instant>,
    last_activity: Instant,
    last_ui: Option<Ui>,
}

fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count - max + 1));
    out
}

fn shown_for(text: &str) -> Duration {
    (Duration::from_secs(4) + Duration::from_millis(50) * text.chars().count() as u32).min(Duration::from_secs(15))
}

impl Machine {
    pub fn new(phrases: Vec<String>, follow_up: Duration, now: Instant) -> Self {
        Self {
            phrases,
            follow_up,
            state: State::Idle,
            paused: false,
            approvals: VecDeque::new(),
            session: None,
            follow_up_until: None,
            last_activity: now,
            last_ui: None,
        }
    }

    pub fn set_follow_up(&mut self, follow_up: Duration) {
        self.follow_up = follow_up;
    }

    fn following(&self, now: Instant) -> bool {
        !self.paused && self.follow_up_until.is_some_and(|until| now < until)
    }

    pub fn ui(&self, now: Instant) -> Ui {
        let (mode, title, detail, approval) = match &self.state {
            State::Idle => (Mode::Idle, "", "", None),
            State::Sleep => (Mode::Sleep, "", "", None),
            State::Listening { transcribing: false, heard, .. } => (Mode::Listening, heard.as_str(), "", None),
            State::Listening { transcribing: true, heard, .. } => (Mode::Transcribing, heard.as_str(), "", None),
            State::Busy { prompt, tool, text, .. } => match (self.approvals.front(), tool) {
                (Some(a), _) => (Mode::Approval, a.tool.as_str(), a.detail.as_str(), Some(a.id)),
                (None, Some(tool)) => (Mode::Working, prompt.as_str(), tool.as_str(), None),
                (None, None) => (Mode::Thinking, prompt.as_str(), text.as_str(), None),
            },
            State::Shown { error, title, detail, .. } => {
                (if *error { Mode::Error } else { Mode::Done }, title.as_str(), detail.as_str(), None)
            }
        };
        let title = if matches!(mode, Mode::Listening | Mode::Transcribing) {
            tail(title, MAX_CAPTION)
        } else {
            truncate(title, MAX_TITLE)
        };
        Ui {
            mode,
            title,
            detail: truncate(detail, MAX_DETAIL),
            approval,
            paused: self.paused,
            follow_up: self.following(now) && matches!(mode, Mode::Idle | Mode::Done),
        }
    }

    pub fn handle(&mut self, event: Event, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        if event != Event::Tick {
            self.last_activity = now;
        }
        match event {
            Event::SpeechStarted => match &mut self.state {
                State::Sleep => self.state = State::Idle,
                State::Listening { until, .. } => *until = now + UTTERANCE_TIMEOUT,
                _ => {}
            },
            Event::Transcribing => self.transcribing(now),
            Event::Transcript(text) => self.transcript(&text, now, &mut fx),
            Event::Partial(text) => {
                if let State::Listening { heard, .. } = &mut self.state {
                    // Without the wake phrase, and an unclear guess never wipes a good one.
                    match wake::hear(&text, &self.phrases) {
                        Heard::Wake(command) | Heard::Speech(command) if !command.is_empty() => *heard = command,
                        _ => {}
                    }
                }
            }
            Event::Typed(text) => {
                if !matches!(self.state, State::Busy { .. }) && !text.trim().is_empty() {
                    self.command(text.trim().to_string(), &mut fx);
                }
            }
            Event::ListenRequested => {
                if self.paused {
                    self.paused = false;
                    fx.push(Effect::Mic(true));
                }
                if !matches!(self.state, State::Busy { .. } | State::Listening { .. }) {
                    self.listen(now, &mut fx);
                }
            }
            Event::CancelRequested => match self.state {
                State::Busy { .. } => self.cancel(&mut fx),
                State::Listening { .. } | State::Shown { .. } => self.state = State::Idle,
                State::Idle | State::Sleep => {}
            },
            Event::PauseToggled => {
                self.paused = !self.paused;
                fx.push(Effect::Mic(!self.paused));
                if self.paused {
                    self.follow_up_until = None;
                    if matches!(self.state, State::Listening { .. }) {
                        self.state = State::Idle;
                    }
                }
            }
            Event::Brain(event) => self.brain(event, now, &mut fx),
            Event::ApprovalRequested { id, tool, detail } => {
                if matches!(self.state, State::Busy { .. }) {
                    self.approvals.push_back(Approval { id, tool, detail });
                    if self.approvals.len() == 1 {
                        fx.push(Effect::Sfx(Sfx::Attention));
                    }
                } else {
                    // Not asked by a command of ours: nobody here can vouch for it.
                    fx.push(Effect::Approve { id, allow: false });
                }
            }
            Event::ApprovalAnswered { id, allow } => {
                if let Some(at) = self.approvals.iter().position(|a| a.id == id) {
                    self.approvals.remove(at);
                    fx.push(Effect::Approve { id, allow });
                    if at == 0 && !self.approvals.is_empty() {
                        fx.push(Effect::Sfx(Sfx::Attention));
                    }
                }
            }
            Event::Problem(message) => {
                if !matches!(self.state, State::Busy { .. }) {
                    self.show(true, "Claude Notch".into(), message, now + ERROR_SHOWN);
                    fx.push(Effect::Sfx(Sfx::Error));
                }
            }
            Event::Tick => self.tick(now),
        }
        let ui = self.ui(now);
        if self.last_ui.as_ref() != Some(&ui) {
            self.last_ui = Some(ui.clone());
            fx.push(Effect::Ui(ui));
        }
        fx
    }

    fn transcribing(&mut self, now: Instant) {
        if self.paused {
            return;
        }
        let following = self.following(now);
        match &mut self.state {
            State::Listening { until, transcribing, .. } => {
                *transcribing = true;
                *until = (*until).max(now + UTTERANCE_TIMEOUT);
            }
            State::Idle | State::Sleep | State::Shown { .. } if following => {
                self.state =
                    State::Listening { until: now + UTTERANCE_TIMEOUT, transcribing: true, heard: String::new() };
            }
            _ => {}
        }
    }

    fn transcript(&mut self, text: &str, now: Instant, fx: &mut Vec<Effect>) {
        if self.paused {
            return;
        }
        let heard = wake::hear(text, &self.phrases);
        match &mut self.state {
            State::Busy { .. } => {
                if matches!(&heard, Heard::Wake(command) if wake::is_cancel(command)) {
                    self.cancel(fx);
                }
            }
            State::Listening { until, transcribing, .. } => match heard {
                Heard::Wake(command) | Heard::Speech(command) if !command.is_empty() => self.command(command, fx),
                // A cough, or "hi Claude" again: keep waiting a little.
                _ => {
                    *transcribing = false;
                    *until = (*until).min(now + LISTEN_TIMEOUT);
                }
            },
            State::Idle | State::Sleep | State::Shown { .. } => match heard {
                Heard::Wake(command) if command.is_empty() => self.listen(now, fx),
                Heard::Wake(command) => {
                    fx.push(Effect::Sfx(Sfx::Wake));
                    self.command(command, fx);
                }
                Heard::Speech(command) if self.following(now) => self.command(command, fx),
                Heard::Speech(_) | Heard::Noise => {}
            },
        }
    }

    fn listen(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        self.state = State::Listening { until: now + LISTEN_TIMEOUT, transcribing: false, heard: String::new() };
        fx.push(Effect::Sfx(Sfx::Wake));
    }

    /// Every command continues the one conversation of this run of the notch; only a restart
    /// starts a new one. The follow-up window decides whether the wake phrase is needed, not this.
    fn command(&mut self, command: String, fx: &mut Vec<Effect>) {
        if wake::is_cancel(&command) {
            self.state = State::Idle;
            self.follow_up_until = None;
            fx.push(Effect::Sfx(Sfx::Cancel));
            return;
        }
        let resume = self.session.clone();
        self.follow_up_until = None;
        self.state = State::Busy {
            prompt: command.clone(),
            tool: None,
            text: String::new(),
            resumed: resume.is_some(),
            started: false,
        };
        fx.push(Effect::StartBrain { prompt: command, resume });
    }

    fn cancel(&mut self, fx: &mut Vec<Effect>) {
        fx.push(Effect::CancelBrain);
        self.deny_all(fx);
        self.state = State::Idle;
        fx.push(Effect::Sfx(Sfx::Cancel));
    }

    fn deny_all(&mut self, fx: &mut Vec<Effect>) {
        fx.extend(self.approvals.drain(..).map(|a| Effect::Approve { id: a.id, allow: false }));
    }

    fn show(&mut self, error: bool, title: String, detail: String, until: Instant) {
        self.state = State::Shown { error, title, detail, until };
    }

    fn brain(&mut self, event: BrainEvent, now: Instant, fx: &mut Vec<Effect>) {
        let State::Busy { prompt, tool, text, resumed, started } = &mut self.state else { return };
        // The conversation could not be loaded (its transcript was deleted, …): Claude Code
        // fails before reporting a session. Forget it and run the same command in a new one.
        let failed_to_load =
            matches!(&event, BrainEvent::Finished { is_error: true, .. } | BrainEvent::Exited { error: Some(_) });
        if failed_to_load && *resumed && !*started {
            *resumed = false;
            self.session = None;
            fx.push(Effect::StartBrain { prompt: prompt.clone(), resume: None });
            return;
        }
        match event {
            BrainEvent::Started { session_id } => {
                *started = true;
                self.session = Some(session_id);
            }
            BrainEvent::Tool { label } => *tool = Some(label),
            BrainEvent::Text(t) => *text = t,
            BrainEvent::Finished { text: answer, is_error, session_id, .. } => {
                let title = std::mem::take(prompt);
                if session_id.is_some() {
                    self.session = session_id;
                }
                let answer = if answer.is_empty() { "Done.".to_string() } else { answer };
                let until = now + shown_for(&answer);
                self.show(is_error, title, answer, until);
                self.deny_all(fx);
                fx.push(Effect::Sfx(if is_error { Sfx::Error } else { Sfx::Done }));
                if !is_error && !self.follow_up.is_zero() {
                    self.follow_up_until = Some(now + self.follow_up);
                }
            }
            BrainEvent::Exited { error } => {
                let title = std::mem::take(prompt);
                let detail = error.unwrap_or_else(|| "Claude stopped without an answer.".into());
                self.show(true, title, detail, now + ERROR_SHOWN);
                self.deny_all(fx);
                fx.push(Effect::Sfx(Sfx::Error));
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        match self.state {
            State::Listening { until, .. } | State::Shown { until, .. } if now >= until => self.state = State::Idle,
            State::Idle if now.duration_since(self.last_activity) >= SLEEP_AFTER && !self.following(now) => {
                self.state = State::Sleep;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "4700cef8-84e7-4458-a427-1d252586e54c";

    struct T {
        m: Machine,
        now: Instant,
    }

    impl T {
        fn new() -> Self {
            Self::with_follow_up(60)
        }

        fn with_follow_up(secs: u64) -> Self {
            let now = Instant::now();
            let phrases = crate::config::Config::default().wake_phrases;
            Self { m: Machine::new(phrases, Duration::from_secs(secs), now), now }
        }

        fn at(&mut self, secs: u64) -> &mut Self {
            self.now += Duration::from_secs(secs);
            self
        }

        fn send(&mut self, event: Event) -> Vec<Effect> {
            self.m.handle(event, self.now)
        }

        fn say(&mut self, text: &str) -> Vec<Effect> {
            self.send(Event::Transcribing);
            self.send(Event::Transcript(text.into()))
        }

        fn brain(&mut self, event: BrainEvent) -> Vec<Effect> {
            self.send(Event::Brain(event))
        }

        fn finish(&mut self, text: &str) -> Vec<Effect> {
            self.brain(BrainEvent::Started { session_id: SID.into() });
            let fx = self.brain(BrainEvent::Finished {
                text: text.into(),
                is_error: false,
                session_id: Some(SID.into()),
                denied: 0,
            });
            self.brain(BrainEvent::Exited { error: None });
            fx
        }

        fn mode(&self) -> Mode {
            self.m.ui(self.now).mode
        }
    }

    fn start(prompt: &str, resume: Option<&str>) -> Effect {
        Effect::StartBrain { prompt: prompt.into(), resume: resume.map(String::from) }
    }

    fn approvals(fx: &[Effect]) -> Vec<(u64, bool)> {
        fx.iter()
            .filter_map(|e| match e {
                Effect::Approve { id, allow } => Some((*id, *allow)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn starts_idle_and_reports_it_once() {
        let mut t = T::new();
        let fx = t.send(Event::Tick);
        assert!(matches!(&fx[..], [Effect::Ui(Ui { mode: Mode::Idle, .. })]));
        assert_eq!(t.send(Event::Tick), Vec::new());
    }

    #[test]
    fn wake_phrase_alone_opens_listening_then_command_runs() {
        let mut t = T::new();
        let fx = t.say("Hi Claude.");
        assert!(fx.contains(&Effect::Sfx(Sfx::Wake)));
        assert_eq!(t.mode(), Mode::Listening);
        t.send(Event::SpeechStarted);
        t.send(Event::Transcribing);
        assert_eq!(t.mode(), Mode::Transcribing);
        let fx = t.send(Event::Transcript("open firefox".into()));
        assert!(fx.contains(&start("open firefox", None)));
        let ui = t.m.ui(t.now);
        assert_eq!((ui.mode, ui.title.as_str()), (Mode::Thinking, "open firefox"));
    }

    #[test]
    fn wake_phrase_with_command_runs_at_once() {
        let mut t = T::new();
        let fx = t.say("Hey Claude, let's code buat project baru");
        assert!(fx.contains(&Effect::Sfx(Sfx::Wake)));
        assert!(fx.contains(&start("let's code buat project baru", None)));
    }

    #[test]
    fn idle_ignores_speech_and_noise() {
        let mut t = T::new();
        t.send(Event::Tick);
        for text in ["I like Claude Code", "Terima kasih telah menonton!", "[BLANK_AUDIO]"] {
            let fx = t.say(text);
            assert!(fx.is_empty(), "{text}: {fx:?}");
        }
        assert_eq!(t.mode(), Mode::Idle);
    }

    #[test]
    fn listening_times_out_and_noise_shortens_it() {
        let mut t = T::new();
        t.send(Event::ListenRequested);
        t.at(7).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Listening);
        t.at(2).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Idle);

        t.send(Event::ListenRequested);
        t.send(Event::SpeechStarted);
        t.at(20).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Listening, "speech extends the wait");
        t.say("(batuk)");
        t.at(9).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Idle);
    }

    #[test]
    fn full_turn_shows_progress_and_answer() {
        let mut t = T::new();
        t.say("hi claude open youtube");
        t.brain(BrainEvent::Started { session_id: SID.into() });
        t.brain(BrainEvent::Tool { label: "$ xdg-open https://youtube.com".into() });
        let ui = t.m.ui(t.now);
        assert_eq!((ui.mode, ui.detail.as_str()), (Mode::Working, "$ xdg-open https://youtube.com"));
        let fx = t.brain(BrainEvent::Finished {
            text: "YouTube is open.".into(),
            is_error: false,
            session_id: Some(SID.into()),
            denied: 0,
        });
        assert!(fx.contains(&Effect::Sfx(Sfx::Done)));
        let ui = t.m.ui(t.now);
        assert_eq!((ui.mode, ui.title.as_str(), ui.detail.as_str()), (Mode::Done, "open youtube", "YouTube is open."));
        assert!(ui.follow_up);
        assert!(t.brain(BrainEvent::Exited { error: None }).is_empty(), "exit after the answer changes nothing");
        t.at(16).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Idle);
    }

    #[test]
    fn follow_up_resumes_the_session_without_wake_phrase() {
        let mut t = T::new();
        t.say("hey claude play a song");
        t.finish("Playing.");
        t.at(20);
        let fx = t.say("turn the volume down");
        assert!(fx.contains(&start("turn the volume down", Some(SID))));
    }

    #[test]
    fn after_the_follow_up_window_the_wake_phrase_is_needed_but_the_conversation_goes_on() {
        let mut t = T::new();
        let fx = t.say("hey claude play a song");
        assert!(fx.contains(&start("play a song", None)), "the first command starts the conversation");
        t.finish("Playing.");
        t.at(61);
        assert_eq!(t.say("turn the volume down"), Vec::new());
        let fx = t.say("hey claude turn the volume down");
        assert!(fx.contains(&start("turn the volume down", Some(SID))));
        t.finish("Done.");
        t.at(3 * 3600);
        let fx = t.say("hey claude what was that song?");
        assert!(fx.contains(&start("what was that song?", Some(SID))), "hours later, still the same conversation");
    }

    #[test]
    fn a_conversation_that_cannot_be_loaded_is_replaced_by_a_new_one() {
        let mut t = T::new();
        t.say("hey claude a");
        t.finish("ok");
        t.say("hey claude b");
        // What Claude Code 2.1 does when the transcript is gone: an error result, no session.
        let fx = t.brain(BrainEvent::Finished {
            text: String::new(),
            is_error: true,
            session_id: Some("00000000-1111-2222-3333-444444444444".into()),
            denied: 0,
        });
        assert_eq!(fx, vec![start("b", None)], "the same command again, in a new conversation");
        // Same when it only exits with an error.
        let mut u = T::new();
        u.say("hey claude a");
        u.finish("ok");
        u.say("hey claude b");
        assert_eq!(u.brain(BrainEvent::Exited { error: Some("No conversation found".into()) }), vec![start("b", None)]);
        assert_eq!(t.mode(), Mode::Thinking);
        t.brain(BrainEvent::Started { session_id: "11111111-2222-3333-4444-555555555555".into() });
        let fx = t.brain(BrainEvent::Exited { error: Some("network".into()) });
        assert_eq!(t.mode(), Mode::Error, "a fresh run that fails is an error, never a loop");
        assert!(!fx.iter().any(|e| matches!(e, Effect::StartBrain { .. })));
        let fx = t.say("hey claude c");
        assert!(fx.contains(&start("c", Some("11111111-2222-3333-4444-555555555555"))));
    }

    #[test]
    fn a_failure_after_loading_keeps_the_conversation() {
        let mut t = T::new();
        t.say("hey claude a");
        t.finish("ok");
        t.say("hey claude b");
        t.brain(BrainEvent::Started { session_id: SID.into() });
        t.brain(BrainEvent::Exited { error: Some("Claude took longer than 300 s".into()) });
        assert_eq!(t.mode(), Mode::Error);
        assert!(t.say("hey claude c").contains(&start("c", Some(SID))));
    }

    #[test]
    fn follow_up_can_be_turned_off() {
        let mut t = T::with_follow_up(0);
        t.say("hey claude play a song");
        t.finish("Playing.");
        assert!(!t.m.ui(t.now).follow_up);
        assert_eq!(t.at(1).say("turn the volume down"), Vec::new());
    }

    #[test]
    fn busy_ignores_speech_but_a_spoken_stop_cancels() {
        let mut t = T::new();
        t.say("hey claude open firefox");
        assert_eq!(t.say("hey claude open chrome"), Vec::new());
        assert!(t.say("stop").is_empty(), "without the wake phrase it is just talk");
        let fx = t.say("hey claude, batal!");
        assert!(fx.contains(&Effect::CancelBrain));
        assert_eq!(t.mode(), Mode::Idle);
        assert!(t.brain(BrainEvent::Exited { error: None }).is_empty(), "late events of a cancelled run");
    }

    #[test]
    fn cancel_word_as_command_runs_nothing() {
        let mut t = T::new();
        t.say("hi claude");
        let fx = t.say("gak jadi");
        assert!(fx.contains(&Effect::Sfx(Sfx::Cancel)));
        assert!(!fx.iter().any(|e| matches!(e, Effect::StartBrain { .. })));
        assert_eq!(t.mode(), Mode::Idle);
    }

    #[test]
    fn approvals_queue_and_answer_in_order() {
        let mut t = T::new();
        t.say("hey claude make a project");
        let fx = t.send(Event::ApprovalRequested { id: 1, tool: "Bash".into(), detail: "mkdir ~/Projects/x".into() });
        assert!(fx.contains(&Effect::Sfx(Sfx::Attention)));
        t.send(Event::ApprovalRequested { id: 2, tool: "Bash".into(), detail: "konsole".into() });
        let ui = t.m.ui(t.now);
        assert_eq!((ui.mode, ui.approval, ui.detail.as_str()), (Mode::Approval, Some(1), "mkdir ~/Projects/x"));

        assert!(approvals(&t.send(Event::ApprovalAnswered { id: 9, allow: true })).is_empty(), "unknown id");
        let fx = t.send(Event::ApprovalAnswered { id: 1, allow: true });
        assert_eq!(approvals(&fx), vec![(1, true)]);
        assert_eq!(t.m.ui(t.now).approval, Some(2));
        let fx = t.send(Event::ApprovalAnswered { id: 2, allow: false });
        assert_eq!(approvals(&fx), vec![(2, false)]);
        assert_eq!(t.mode(), Mode::Thinking);
    }

    #[test]
    fn approvals_outside_a_command_are_denied() {
        let mut t = T::new();
        let fx = t.send(Event::ApprovalRequested { id: 5, tool: "Bash".into(), detail: "rm -rf ~".into() });
        assert_eq!(approvals(&fx), vec![(5, false)]);
        assert_eq!(t.mode(), Mode::Idle);
    }

    #[test]
    fn pending_approvals_are_denied_when_the_run_ends_or_is_cancelled() {
        let mut t = T::new();
        t.say("hey claude a");
        t.send(Event::ApprovalRequested { id: 1, tool: "Bash".into(), detail: "x".into() });
        let fx = t.send(Event::CancelRequested);
        assert_eq!(approvals(&fx), vec![(1, false)]);
        assert!(fx.contains(&Effect::CancelBrain));

        t.say("hey claude b");
        t.send(Event::ApprovalRequested { id: 2, tool: "Bash".into(), detail: "y".into() });
        let fx = t.brain(BrainEvent::Exited { error: Some("crashed".into()) });
        assert_eq!(approvals(&fx), vec![(2, false)]);
        let ui = t.m.ui(t.now);
        assert_eq!((ui.mode, ui.detail.as_str()), (Mode::Error, "crashed"));
    }

    #[test]
    fn error_result_shows_error_without_follow_up() {
        let mut t = T::new();
        t.say("hey claude a");
        let fx =
            t.brain(BrainEvent::Finished { text: "Rate limited".into(), is_error: true, session_id: None, denied: 0 });
        assert!(fx.contains(&Effect::Sfx(Sfx::Error)));
        let ui = t.m.ui(t.now);
        assert_eq!((ui.mode, ui.follow_up), (Mode::Error, false));
    }

    #[test]
    fn pause_mutes_the_mic_and_listen_resumes_it() {
        let mut t = T::new();
        let fx = t.send(Event::PauseToggled);
        assert!(fx.contains(&Effect::Mic(false)));
        assert!(t.m.ui(t.now).paused);
        assert!(!t.say("hey claude open firefox").iter().any(|e| matches!(e, Effect::StartBrain { .. })));
        let fx = t.send(Event::ListenRequested);
        assert!(fx.contains(&Effect::Mic(true)));
        assert_eq!(t.mode(), Mode::Listening);
        assert!(!t.m.ui(t.now).paused);
    }

    #[test]
    fn sleeps_when_nothing_happens_and_wakes_on_speech() {
        let mut t = T::new();
        t.at(119).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Idle);
        t.at(2).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Sleep);
        t.send(Event::SpeechStarted);
        assert_eq!(t.mode(), Mode::Idle);
    }

    #[test]
    fn problems_show_but_never_interrupt_a_run() {
        let mut t = T::new();
        t.send(Event::Problem("No microphone".into()));
        assert_eq!(t.mode(), Mode::Error);
        t.at(9).send(Event::Tick);
        assert_eq!(t.mode(), Mode::Idle);
        t.say("hey claude a");
        t.send(Event::Problem("x".into()));
        assert_eq!(t.mode(), Mode::Thinking);
    }

    #[test]
    fn typed_commands_need_no_wake_phrase_and_no_microphone() {
        let mut t = T::new();
        t.send(Event::PauseToggled);
        let fx = t.send(Event::Typed("  open youtube ".into()));
        assert!(fx.contains(&start("open youtube", None)));
        assert_eq!(t.send(Event::Typed("open chrome".into())), Vec::new(), "busy");
        t.finish("ok");
        assert_eq!(t.send(Event::Typed("   ".into())), Vec::new());
        let fx = t.send(Event::Typed("go on".into()));
        assert!(fx.contains(&start("go on", Some(SID))), "typed commands continue the conversation too: {fx:?}");
    }

    #[test]
    fn live_captions_while_listening_only() {
        let mut t = T::new();
        t.send(Event::Partial("hey claude open".into()));
        assert_eq!(t.m.ui(t.now).title, "", "idle: no caption");
        t.send(Event::ListenRequested);
        t.send(Event::Partial("open you".into()));
        assert_eq!(t.m.ui(t.now).title, "open you");
        t.send(Event::Partial("Hey Claude, open YouTube".into()));
        assert_eq!(t.m.ui(t.now).title, "open YouTube", "wake phrase is not shown");
        t.send(Event::Partial("[BLANK_AUDIO]".into()));
        assert_eq!(t.m.ui(t.now).title, "open YouTube", "noise keeps the last good caption");
        t.send(Event::Transcribing);
        assert_eq!(t.m.ui(t.now), Ui { mode: Mode::Transcribing, title: "open YouTube".into(), ..t.m.ui(t.now) });
        let fx = t.send(Event::Transcript("open YouTube".into()));
        assert!(fx.contains(&start("open YouTube", None)), "the final transcript decides, not the caption");
        t.finish("ok");
        t.at(20).send(Event::ListenRequested);
        assert_eq!(t.m.ui(t.now).title, "", "a new listen starts with an empty caption");
        let long = format!("{} open youtube", "word ".repeat(40));
        t.send(Event::Partial(long));
        let title = t.m.ui(t.now).title;
        assert!(title.starts_with('…') && title.ends_with("open youtube"), "{title}");
        assert_eq!(title.chars().count(), MAX_CAPTION);
    }

    #[test]
    fn long_text_is_truncated_for_the_notch() {
        let mut t = T::new();
        t.say("hey claude a");
        t.brain(BrainEvent::Finished { text: "x".repeat(5000), is_error: false, session_id: None, denied: 0 });
        assert_eq!(t.m.ui(t.now).detail.chars().count(), MAX_DETAIL);
    }
}
