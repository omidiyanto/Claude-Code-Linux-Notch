//! Speech to text on this computer with whisper.cpp; audio never leaves the machine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// A sentence with the names Whisper tends to misspell. A list of bare names was tried and made
/// things worse ("Claude" + "Chromium" became "Chroude").
const PROMPT: &str = "Hey Claude, open Chromium and play a song on YouTube.";
/// whisper.cpp ignores audio shorter than one second.
const MIN_SAMPLES: usize = 17_600;
const NO_SPEECH: f32 = 0.6;

/// The encoder window for this much audio. Whisper always encodes 30 s (1500 frames of 20 ms);
/// a voice command is a few seconds, so encoding only what is there (plus a margin, below which
/// accuracy drops) makes recognition several times faster.
fn audio_ctx(samples: usize) -> i32 {
    i32::try_from(samples / 320 + 128).map_or(1500, |frames| frames.clamp(256, 1500))
}

/// A GPU ggml can run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    Dedicated,
    Integrated,
}

/// whisper.cpp's `gpu_device`: the first dedicated GPU. Left at 0, whisper.cpp takes the first
/// GPU of any kind, and on a laptop that is the integrated one (Intel before NVIDIA here).
fn pick_gpu(gpus: &[Gpu]) -> i32 {
    gpus.iter().position(|g| *g == Gpu::Dedicated).and_then(|i| i32::try_from(i).ok()).unwrap_or(0)
}

/// The GPUs this build can use, in ggml's order; empty for a CPU-only build.
fn gpus() -> Vec<Gpu> {
    use whisper_rs_sys as sys;
    // SAFETY: read-only queries of ggml's device registry, which sets itself up on first use;
    // every index is below the count it reports.
    let count = unsafe { sys::ggml_backend_dev_count() };
    (0..count)
        .filter_map(|i| match unsafe { sys::ggml_backend_dev_type(sys::ggml_backend_dev_get(i)) } {
            sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU => Some(Gpu::Dedicated),
            sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU => Some(Gpu::Integrated),
            _ => None,
        })
        .collect()
}

pub struct Stt {
    state: WhisperState,
    /// One language: always that one. Several: the most likely of them, per utterance.
    languages: Vec<String>,
    threads: i32,
}

/// Whisper's probability for a language code, from its full table.
fn probability(probs: &[f32], lang: &str) -> f32 {
    whisper_rs::get_lang_id(lang)
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| probs.get(i))
        .copied()
        .unwrap_or(0.0)
}

/// The allowed language Whisper finds most likely. Left free to choose among all ~100, it may
/// hear accented English as another language and then write a translation instead of what was said.
pub fn pick_language(prob: impl Fn(&str) -> f32, allowed: &[String]) -> String {
    allowed.iter().max_by(|a, b| prob(a).total_cmp(&prob(b))).cloned().unwrap_or_else(|| "en".into())
}

#[derive(Debug)]
pub enum SttEvent {
    Failed(String),
    Started,
    /// A live caption of the utterance still being spoken.
    Partial(String),
    Text(String),
}

impl Stt {
    pub fn load(model: &Path, languages: Vec<String>) -> Result<Self, String> {
        if !model.is_file() {
            return Err(format!("Speech model missing: {}. Run scripts/download-model.sh", model.display()));
        }
        // Routes whisper.cpp's chatter away from stderr.
        whisper_rs::install_logging_hooks();
        let gpus = gpus();
        let mut params = WhisperContextParameters::default();
        // flash_attn stays off: it saves ~120 MB but garbled short commands with small-q5_1 (measured).
        params.gpu_device(pick_gpu(&gpus));
        crate::log(&match gpus.get(usize::try_from(pick_gpu(&gpus)).unwrap_or(0)) {
            Some(gpu) => format!("speech recognition on the GPU ({gpu:?})"),
            None => "speech recognition on the CPU".into(),
        });
        let ctx =
            WhisperContext::new_with_params(model, params).map_err(|e| format!("loading {}: {e}", model.display()))?;
        let state = ctx.create_state().map_err(|e| format!("speech model: {e}"))?;
        let threads = std::thread::available_parallelism().map_or(4, |n| i32::try_from(n.get().min(8)).unwrap_or(4));
        Ok(Self { state, languages, threads })
    }

    fn detect(&mut self, audio: &[f32]) -> Result<String, String> {
        let threads = usize::try_from(self.threads).unwrap_or(4);
        self.state.pcm_to_mel(audio, threads).map_err(|e| format!("speech recognition: {e}"))?;
        let (_, probs) = self.state.lang_detect(0, threads).map_err(|e| format!("language detection: {e}"))?;
        Ok(pick_language(|lang| probability(&probs, lang), &self.languages))
    }

    pub fn transcribe(&mut self, audio: &[f32]) -> Result<String, String> {
        let padded;
        let audio = if audio.len() < MIN_SAMPLES {
            padded = [audio, &vec![0.0; MIN_SAMPLES - audio.len()]].concat();
            &padded
        } else {
            audio
        };
        let language = match &self.languages[..] {
            [only] => only.clone(),
            _ => self.detect(audio)?,
        };
        let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        p.set_language(Some(&language));
        p.set_n_threads(self.threads);
        p.set_translate(false);
        p.set_no_context(true);
        p.set_print_special(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_timestamps(false);
        p.set_suppress_blank(true);
        p.set_suppress_nst(true);
        p.set_no_speech_thold(NO_SPEECH);
        p.set_initial_prompt(PROMPT);
        p.set_audio_ctx(audio_ctx(audio.len()));
        self.state.full(p, audio).map_err(|e| format!("speech recognition: {e}"))?;
        let mut text = String::new();
        for segment in self.state.as_iter() {
            if segment.no_speech_probability() > NO_SPEECH {
                continue;
            }
            if let Ok(t) = segment.to_str_lossy() {
                text.push_str(&t);
            }
        }
        Ok(text.trim().to_string())
    }
}

/// How much of an utterance the app needs right now; the driver sets it from the notch state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Need {
    /// Idle or busy: only whether it starts with the wake phrase.
    Wake = 0,
    /// Just after an answer: a short follow-up command, or the wake phrase.
    FollowUp = 1,
    /// Listening after "hi Claude" or a click: every word.
    Everything = 2,
}

impl Need {
    pub fn from_u8(value: u8) -> Self {
        match value {
            2 => Self::Everything,
            1 => Self::FollowUp,
            _ => Self::Wake,
        }
    }
}

/// The wake phrase comes first, so the first seconds are enough to hear it.
const PEEK: usize = 3 * 16_000;
/// A longer follow-up is more likely music or a conversation than a command.
const MAX_FOLLOW_UP: usize = 12 * 16_000;

/// Whether to transcribe only the start first. Music or a long conversation in the room then
/// costs one short pass per utterance instead of a full one.
fn peek_first(need: Need, samples: usize) -> bool {
    match need {
        Need::Everything => false,
        Need::FollowUp if samples <= MAX_FOLLOW_UP => false,
        // Up to 4 s, one pass over everything costs about as much as peeking.
        _ => samples > PEEK + 16_000,
    }
}

/// The text to hand to the state machine. After a peek without the wake phrase it is empty
/// (noise), so a partial transcript can never be taken for a follow-up command.
fn recognize(
    transcribe: &mut impl FnMut(&[f32]) -> Result<String, String>,
    audio: &[f32],
    need: Need,
    phrases: &[String],
) -> Result<String, String> {
    if !peek_first(need, audio.len()) {
        return transcribe(audio);
    }
    let head = transcribe(&audio[..PEEK])?;
    if matches!(crate::wake::hear(&head, phrases), crate::wake::Heard::Wake(_)) {
        transcribe(audio)
    } else {
        Ok(String::new())
    }
}

/// Where the microphone hands audio to recognition.
#[derive(Clone)]
pub struct Feed {
    finals: mpsc::SyncSender<Vec<f32>>,
    partial: Arc<Mutex<Option<Vec<f32>>>>,
}

impl Feed {
    /// A finished utterance. False when recognition is still busy with the previous one.
    pub fn utterance(&self, audio: Vec<f32>) -> bool {
        self.finals.try_send(audio).is_ok()
    }

    /// The utterance so far. Replaces an older snapshot nobody has transcribed yet, so captions
    /// never fall behind; finished utterances always go first.
    pub fn partial(&self, audio: Vec<f32>) {
        *self.partial.lock().unwrap_or_else(PoisonError::into_inner) = Some(audio);
    }
}

/// Loads the model and transcribes on a thread of its own. While it is busy, new utterances
/// are refused (`try_send` fails) rather than queued.
pub fn spawn(
    model: PathBuf,
    languages: Vec<String>,
    phrases: Vec<String>,
    need: Arc<AtomicU8>,
    mut on: impl FnMut(SttEvent) + Send + 'static,
) -> Feed {
    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(1);
    let partial = Arc::new(Mutex::new(None::<Vec<f32>>));
    let feed = Feed { finals: tx, partial: partial.clone() };
    let started = std::thread::Builder::new().name("stt".into()).spawn(move || {
        // Recognition is background work: the desktop comes first. whisper.cpp's worker
        // threads inherit this niceness.
        // SAFETY: plain syscalls about this thread's own id.
        if let Ok(tid) = libc::id_t::try_from(unsafe { libc::gettid() }) {
            // SAFETY: as above.
            unsafe { libc::setpriority(libc::PRIO_PROCESS, tid, 10) };
        }
        let mut stt = match Stt::load(&model, languages) {
            Ok(stt) => stt,
            Err(e) => return on(SttEvent::Failed(e)),
        };
        // The first run allocates whisper.cpp's buffers and takes ~3× longer; pay that now,
        // not on the user's first command.
        if let Err(e) = stt.transcribe(&vec![0.0; MIN_SAMPLES]) {
            return on(SttEvent::Failed(e));
        }
        let take_partial = || partial.lock().unwrap_or_else(PoisonError::into_inner).take();
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(audio) => {
                    take_partial(); // older than this utterance
                    on(SttEvent::Started);
                    let need = Need::from_u8(need.load(Ordering::Relaxed));
                    match recognize(&mut |a| stt.transcribe(a), &audio, need, &phrases) {
                        Ok(text) => on(SttEvent::Text(text)),
                        Err(e) => crate::log(&e),
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(audio) = take_partial() {
                        match stt.transcribe(&audio) {
                            Ok(text) => on(SttEvent::Partial(text)),
                            Err(e) => crate::log(&e),
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    if let Err(e) = started {
        crate::log(&format!("speech thread: {e}"));
    }
    feed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Resampler;
    use crate::wake::{self, Heard};

    /// 16-bit PCM mono WAV, as espeak-ng writes it.
    fn read_wav(path: &Path) -> (u32, Vec<f32>) {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let mut at = 12;
        while &bytes[at..at + 4] != b"data" {
            at += 8 + u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        }
        let samples = bytes[at + 8..].as_chunks::<2>().0.iter().map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0);
        (rate, samples.collect())
    }

    fn speak(voice: &str, sentence: &str) -> Vec<f32> {
        let wav = std::env::temp_dir().join(format!("claude-notch-stt-{voice}-{}.wav", std::process::id()));
        let status = std::process::Command::new("espeak-ng")
            .args(["-v", voice, "-s", "140", "-w"])
            .arg(&wav)
            .arg(sentence)
            .status()
            .expect("espeak-ng is needed for this test");
        assert!(status.success());
        let (rate, samples) = read_wav(&wav);
        std::fs::remove_file(&wav).unwrap();
        let mut audio = Vec::new();
        Resampler::new(rate).push(&samples, &mut audio);
        audio
    }

    fn hear_spoken(voice: &str, sentence: &str) -> Heard {
        let audio = speak(voice, sentence);
        let mut stt = test_stt();
        let started = std::time::Instant::now();
        let text = stt.transcribe(&audio).unwrap();
        crate::log(&format!("{voice}: {sentence:?} -> {text:?} in {:?}", started.elapsed()));
        wake::hear(&text, &crate::config::Config::default().wake_phrases)
    }

    /// A fake recognizer: says `head` for the peek, `full` for anything longer, and counts calls.
    fn fake(
        head: &'static str,
        full: &'static str,
        calls: &mut Vec<usize>,
    ) -> impl FnMut(&[f32]) -> Result<String, String> {
        move |audio| {
            calls.push(audio.len() / 16_000);
            Ok(if audio.len() == PEEK { head } else { full }.to_string())
        }
    }

    fn run(need: Need, secs: usize, head: &'static str, full: &'static str) -> (String, Vec<usize>) {
        let phrases = crate::config::Config::default().wake_phrases;
        let mut calls = Vec::new();
        let text = recognize(&mut fake(head, full, &mut calls), &vec![0.0; secs * 16_000], need, &phrases).unwrap();
        (text, calls)
    }

    #[test]
    fn short_utterances_take_one_full_pass() {
        for need in [Need::Wake, Need::FollowUp, Need::Everything] {
            assert_eq!(run(need, 4, "-", "hey claude open youtube"), ("hey claude open youtube".into(), vec![4]));
        }
    }

    #[test]
    fn long_sound_while_idle_costs_one_peek() {
        assert_eq!(run(Need::Wake, 30, "la la la", "unused"), (String::new(), vec![3]));
        assert_eq!(
            run(Need::Wake, 20, "hey claude", "hey claude open youtube and"),
            ("hey claude open youtube and".into(), vec![3, 20])
        );
    }

    #[test]
    fn long_follow_up_is_never_a_partial_command() {
        assert_eq!(run(Need::FollowUp, 10, "-", "turn it down"), ("turn it down".into(), vec![10]));
        // Music lyrics after an answer: only checked for the wake phrase, and nothing reaches Claude.
        assert_eq!(run(Need::FollowUp, 25, "subo no siento", "unused"), (String::new(), vec![3]));
    }

    #[test]
    fn listening_takes_every_word() {
        assert_eq!(
            run(Need::Everything, 25, "-", "a long dictated command"),
            ("a long dictated command".into(), vec![25])
        );
    }

    #[test]
    fn need_round_trips_through_its_byte() {
        for need in [Need::Wake, Need::FollowUp, Need::Everything] {
            assert_eq!(Need::from_u8(need as u8), need);
        }
        assert_eq!(Need::from_u8(200), Need::Wake);
    }

    /// The default model, or the one in `CLAUDE_NOTCH_TEST_MODEL`, recognizing English.
    fn test_stt() -> Stt {
        let model = std::env::var_os("CLAUDE_NOTCH_TEST_MODEL")
            .map_or_else(|| crate::config::Config::default().model_path(), PathBuf::from);
        Stt::load(&model, vec!["en".into()]).unwrap()
    }

    fn probs(table: &[(&str, f32)]) -> impl Fn(&str) -> f32 {
        let table: Vec<(String, f32)> = table.iter().map(|(l, p)| ((*l).to_string(), *p)).collect();
        move |lang| table.iter().find(|(l, _)| l == lang).map_or(0.0, |(_, p)| *p)
    }

    #[test]
    fn prefers_the_dedicated_gpu() {
        assert_eq!(pick_gpu(&[Gpu::Integrated, Gpu::Dedicated]), 1);
        assert_eq!(pick_gpu(&[Gpu::Dedicated, Gpu::Integrated]), 0);
        assert_eq!(pick_gpu(&[Gpu::Integrated]), 0);
        assert_eq!(pick_gpu(&[]), 0);
    }

    #[test]
    fn picks_the_most_likely_allowed_language() {
        let en_de: Vec<String> = vec!["en".into(), "de".into()];
        assert_eq!(pick_language(probs(&[("en", 0.6), ("de", 0.3)]), &en_de), "en");
        assert_eq!(pick_language(probs(&[("en", 0.3), ("de", 0.6)]), &en_de), "de");
        // Accented English that Whisper rates as French first: still never French.
        assert_eq!(pick_language(probs(&[("fr", 0.9), ("en", 0.06), ("de", 0.03)]), &en_de), "en");
        assert_eq!(pick_language(probs(&[]), &["de".to_string()]), "de");
        assert_eq!(pick_language(probs(&[("en", 1.0)]), &[]), "en");
    }

    #[test]
    fn probability_reads_whispers_table() {
        let mut table = vec![0.0; 100];
        table[usize::try_from(whisper_rs::get_lang_id("de").unwrap()).unwrap()] = 0.7;
        assert!((probability(&table, "de") - 0.7).abs() < f32::EPSILON);
        assert!(probability(&table, "xx").abs() < f32::EPSILON);
        assert!(probability(&[], "en").abs() < f32::EPSILON);
    }

    #[test]
    fn missing_model_is_a_clear_error() {
        let Err(e) = Stt::load(Path::new("/nonexistent/ggml.bin"), vec!["en".into()]) else { panic!("loaded nothing") };
        assert!(e.contains("download-model.sh"), "{e}");
    }

    #[test]
    #[ignore = "needs the downloaded speech model and espeak-ng: cargo test -- --ignored"]
    fn hears_wake_phrase_and_command_end_to_end() {
        let Heard::Wake(command) = hear_spoken("en-us", "Hey Claude, open Firefox.") else { panic!("no wake") };
        assert_eq!(wake::normalize(&command), ["open", "firefox"]);
    }

    #[test]
    #[ignore = "needs the downloaded speech model and espeak-ng: cargo test -- --ignored"]
    fn warm_recognition_meets_the_latency_target() {
        let audio = speak("en-us", "Hey Claude, turn the volume down a little.");
        let mut stt = test_stt();
        let mut times = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            stt.transcribe(&audio).unwrap();
            times.push(started.elapsed());
        }
        let rss = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).map(str::to_string))
            .unwrap_or_default();
        crate::log(&format!("{} s of audio, recognition took {times:?}; {rss}", audio.len() / 16_000));
        // PRD: under 3 s from the end of speech to text, on CPU.
        assert!(times[1..].iter().all(|t| t.as_secs_f32() < 3.0), "{times:?}");
    }

    #[test]
    #[ignore = "needs the downloaded speech model and espeak-ng: cargo test -- --ignored"]
    fn live_captions_grow_while_speaking() {
        use crate::vad::{FRAME, Segmenter, VadEvent};
        let mut audio = speak("en-us", "Open Firefox, then play some relaxing music on YouTube please.");
        audio.extend(vec![0.0; 16_000 * 2]);
        let mut stt = test_stt();
        let mut segmenter = Segmenter::new(0.01);
        let mut captions = Vec::new();
        let mut last = String::new();
        for frame in audio.as_chunks::<FRAME>().0 {
            match segmenter.push(frame) {
                Some(VadEvent::Partial(so_far)) => {
                    let started = std::time::Instant::now();
                    let caption = stt.transcribe(&so_far).unwrap();
                    captions.push((so_far.len() / 1600, caption, started.elapsed()));
                }
                Some(VadEvent::Utterance(all)) => last = stt.transcribe(&all).unwrap(),
                _ => {}
            }
        }
        for (tenths, caption, took) in &captions {
            crate::log(&format!("caption at {:.1} s ({took:?}): {caption:?}", *tenths as f32 / 10.0));
        }
        crate::log(&format!("final: {last:?}"));
        assert!(captions.len() >= 2, "{captions:?}");
        let words = |t: &str| wake::normalize(t).len();
        assert!(captions.windows(2).all(|w| words(&w[1].1) >= words(&w[0].1)), "captions must grow: {captions:?}");
        assert!(wake::normalize(&last).contains(&"youtube".to_string()), "{last:?}");
    }

    #[test]
    #[ignore = "needs the downloaded speech model and espeak-ng: cargo test -- --ignored"]
    fn silence_is_not_a_command() {
        let mut stt = test_stt();
        let text = stt.transcribe(&vec![0.0; 16_000 * 2]).unwrap();
        assert_eq!(wake::hear(&text, &crate::config::Config::default().wake_phrases), Heard::Noise, "{text:?}");
    }
}
