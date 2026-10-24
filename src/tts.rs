//! Spoken answers: Kokoro-82M (ONNX) through ONNX Runtime, on an NVIDIA GPU in the `tts-cuda`
//! build and on the CPU otherwise. English only: phonemes come from espeak-ng's en-us, the sound
//! goes out through `pw-play`. Nothing is written to disk.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, PoisonError};

use ort::session::Session;
use ort::value::Tensor;

pub const RATE: u32 = 24_000;
/// One style vector per input length.
const STYLE: usize = 256;
/// Kokoro reads at most 510 phonemes at a time (512 tokens with the two pads).
const MAX_TOKENS: usize = 510;
/// Longer answers are cut: a voice notch reads a sentence or two, not an essay.
const MAX_TEXT: usize = 1000;
/// Clause ends that Kokoro pauses on; espeak-ng drops them, so they are put back.
const PUNCTUATION: &[char] = &['.', ',', '!', '?', ';', ':'];
/// espeak-ng's diphthongs and affricates (written with a tie) as the single letters Kokoro was trained on.
const SWAPS: &[(&str, &str)] = &[
    ("a^ɪ", "I"),
    ("a^ʊ", "W"),
    ("d^ʒ", "ʤ"),
    ("d^z", "ʣ"),
    ("e^ɪ", "A"),
    ("o^ʊ", "O"),
    ("ə^ʊ", "Q"),
    ("t^ʃ", "ʧ"),
    ("t^s", "ʦ"),
    ("ɔ^ɪ", "Y"),
];

/// A voice file name under `voices/`: two letters (language and gender), `_`, a name.
pub fn valid_voice(name: &str) -> bool {
    let Some((prefix, rest)) = name.split_once('_') else { return false };
    prefix.len() == 2
        && prefix.bytes().all(|b| b.is_ascii_lowercase())
        && (1..=30).contains(&rest.len())
        && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Splits text into clauses, each with the punctuation that ended it. A mark counts only before a
/// space or the end, so "youtube.com" and "3.5" stay whole.
fn clauses(text: &str) -> Vec<(String, Option<char>)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if PUNCTUATION.contains(&c) && chars.peek().is_none_or(|n| n.is_whitespace()) {
            out.push((std::mem::take(&mut current), Some(c)));
        } else {
            current.push(c);
        }
    }
    out.push((current, None));
    out.retain(|(clause, mark)| clause.chars().any(char::is_alphanumeric) || mark.is_some());
    out
}

/// espeak-ng's IPA (with `--tie=^`) in Kokoro's spelling.
fn to_kokoro(ipa: &str) -> String {
    let mut out = ipa.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in SWAPS {
        out = out.replace(from, to);
    }
    out.replace('^', "")
}

fn espeak(clause: &str) -> Result<String, String> {
    // Through stdin: a clause starting with "-" must not read as an option.
    let mut child = Command::new("espeak-ng")
        .args(["-q", "--ipa", "--tie=^", "-v", "en-us"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("espeak-ng: {e} (install espeak-ng)"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(clause.as_bytes()).map_err(|e| format!("espeak-ng: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("espeak-ng: {e}"))?;
    if !out.status.success() {
        return Err(format!("espeak-ng failed ({})", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Each clause as Kokoro phonemes, its punctuation kept.
fn phonemize(text: &str) -> Result<Vec<String>, String> {
    let text: String = text.chars().take(MAX_TEXT).collect();
    let mut out = Vec::new();
    for (clause, mark) in clauses(&text) {
        let mut phonemes =
            if clause.chars().any(char::is_alphanumeric) { to_kokoro(&espeak(&clause)?) } else { String::new() };
        phonemes.extend(mark);
        phonemes.push(' ');
        out.push(phonemes);
    }
    Ok(out)
}

fn tokens(phonemes: &str, vocab: &HashMap<char, i64>) -> Vec<i64> {
    phonemes.chars().filter_map(|c| vocab.get(&c).copied()).collect()
}

/// Packs whole clauses into runs of at most `max` tokens; a clause longer than that is cut.
fn batches(clauses: Vec<Vec<i64>>, max: usize) -> Vec<Vec<i64>> {
    let mut out: Vec<Vec<i64>> = Vec::new();
    for mut clause in clauses {
        clause.truncate(max);
        match out.last_mut() {
            Some(last) if last.len() + clause.len() <= max => last.extend(clause),
            _ => out.push(clause),
        }
    }
    out.retain(|b| !b.is_empty());
    out
}

fn ort_error(e: impl std::fmt::Display) -> String {
    format!("Kokoro: {e}")
}

/// ONNX Runtime allows one environment per process.
fn environment() -> Result<&'static ort::environment::Environment, String> {
    static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let env = ort::init().build().map_err(ort_error)?;
    Ok(ENV.get_or_init(|| env))
}

/// The model on the GPU when this build and the machine can, else on the CPU; says which.
fn open_session(model: &Path) -> Result<Session, String> {
    #[cfg(feature = "tts-cuda")]
    {
        // ONNX Runtime would fall back to the CPU silently; fail instead, to say why.
        let cuda = Session::builder(environment()?)
            .map_err(ort_error)?
            .with_execution_providers([ort::ep::CUDA::default().build().error_on_failure()])
            .map_err(ort_error)
            .and_then(|mut builder| builder.commit_from_file(model).map_err(ort_error));
        match cuda {
            Ok(session) => {
                crate::log("voice on the GPU (CUDA)");
                return Ok(session);
            }
            Err(e) => crate::log(&format!("voice on the CPU, CUDA is not usable: {e}")),
        }
    }
    let session = Session::builder(environment()?).map_err(ort_error)?.commit_from_file(model).map_err(ort_error)?;
    #[cfg(not(feature = "tts-cuda"))]
    crate::log("voice on the CPU");
    Ok(session)
}

/// Kokoro's files, as `scripts/download-model.sh kokoro` installs them.
pub fn model_dir() -> PathBuf {
    crate::config::models_dir().join("kokoro")
}

pub struct Kokoro {
    session: Session,
    vocab: HashMap<char, i64>,
    dir: PathBuf,
}

impl Kokoro {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let model = dir.join("model.onnx");
        if !model.is_file() {
            return Err(format!("Voice model missing: run scripts/download-model.sh kokoro ({})", model.display()));
        }
        let tokenizer = dir.join("tokenizer.json");
        let json: serde_json::Value = std::fs::read_to_string(&tokenizer)
            .map_err(|e| e.to_string())
            .and_then(|text| serde_json::from_str(&text).map_err(|e| e.to_string()))
            .map_err(|e| format!("{}: {e}", tokenizer.display()))?;
        let vocab: HashMap<char, i64> = json["model"]["vocab"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(symbol, id)| {
                let mut chars = symbol.chars();
                match (chars.next(), chars.next(), id.as_i64()) {
                    (Some(c), None, Some(id)) => Some((c, id)),
                    _ => None,
                }
            })
            .collect();
        if vocab.is_empty() {
            return Err(format!("{}: no vocabulary", tokenizer.display()));
        }
        let session = open_session(&model)?;
        let mut kokoro = Self { session, vocab, dir: dir.to_path_buf() };
        // The first run sets up the GPU kernels: pay for it now, not on the first answer.
        kokoro.run(&[0], &[0.0; STYLE * 2])?;
        Ok(kokoro)
    }

    /// The answer as 24 kHz mono samples.
    pub fn speak(&mut self, text: &str, voice: &str) -> Result<Vec<f32>, String> {
        if !valid_voice(voice) {
            return Err(format!("{voice:?} is not a Kokoro voice name"));
        }
        let path = self.dir.join("voices").join(format!("{voice}.bin"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let styles: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        if styles.len() < STYLE * 2 || !styles.len().is_multiple_of(STYLE) {
            return Err(format!("{}: not a Kokoro voice", path.display()));
        }
        let max = MAX_TOKENS.min(styles.len() / STYLE - 1);
        let clauses = phonemize(text)?;
        let mut audio = Vec::new();
        for batch in batches(clauses.iter().map(|c| tokens(c, &self.vocab)).collect(), max) {
            audio.extend(self.run(&batch, &styles)?);
        }
        Ok(audio)
    }

    fn run(&mut self, tokens: &[i64], styles: &[f32]) -> Result<Vec<f32>, String> {
        let n = tokens.len();
        let style = styles.get(n * STYLE..(n + 1) * STYLE).ok_or("Kokoro: the voice file is too short")?;
        let mut ids = Vec::with_capacity(n + 2);
        ids.push(0);
        ids.extend_from_slice(tokens);
        ids.push(0);
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => Tensor::from_array(([1, n + 2], ids)).map_err(ort_error)?,
                "style" => Tensor::from_array(([1, STYLE], style.to_vec())).map_err(ort_error)?,
                "speed" => Tensor::from_array(([1], vec![1.0_f32])).map_err(ort_error)?,
            ])
            .map_err(ort_error)?;
        let (_, audio) = outputs[0].try_extract_tensor::<f32>().map_err(ort_error)?;
        Ok(audio.to_vec())
    }
}

/// Loads Kokoro on first use and keeps it loaded; one answer at a time.
pub struct Speaker {
    dir: PathBuf,
    kokoro: Mutex<Option<Kokoro>>,
}

impl Speaker {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, kokoro: Mutex::new(None) }
    }

    /// Blocking: loads the model if needed, then synthesizes. An empty text only loads it.
    pub fn synthesize(&self, text: &str, voice: &str) -> Result<Vec<f32>, String> {
        let mut kokoro = self.kokoro.lock().unwrap_or_else(PoisonError::into_inner);
        if kokoro.is_none() {
            *kokoro = Some(Kokoro::load(&self.dir)?);
        }
        match kokoro.as_mut() {
            Some(kokoro) if !text.trim().is_empty() => kokoro.speak(text, voice),
            _ => Ok(Vec::new()),
        }
    }
}

/// Plays 24 kHz mono samples; dropping the future stops the sound.
pub async fn play(samples: &[f32]) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let rate = RATE.to_string();
    let mut child = tokio::process::Command::new("pw-play")
        .args(["--raw", "--format", "s16", "--channels", "1", "--rate", &rate, "--media-role", "Communication", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("pw-play: {e} (PipeWire is needed to speak)"))?;
    let pcm: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&pcm).await.map_err(|e| format!("pw-play: {e}"))?;
    }
    let status = child.wait().await.map_err(|e| format!("pw-play: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("pw-play failed ({status})")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_names_are_checked() {
        for good in ["af_heart", "am_michael", "bf_emma", "zf_xiaobei", "af_v0"] {
            assert!(valid_voice(good), "{good}");
        }
        for bad in ["", "af", "afheart", "AF_heart", "a_heart", "af_", "af_../x", "af_heart.bin", "../af_heart"] {
            assert!(!valid_voice(bad), "{bad}");
        }
    }

    #[test]
    fn splits_clauses_at_punctuation_followed_by_space() {
        let c = clauses("Hello! Open youtube.com, version 3.5 now.");
        let texts: Vec<_> = c.iter().map(|(t, m)| (t.trim(), *m)).collect();
        assert_eq!(texts, [("Hello", Some('!')), ("Open youtube.com", Some(',')), ("version 3.5 now", Some('.'))]);
        assert_eq!(clauses("no ending"), [("no ending".to_string(), None)]);
        assert_eq!(clauses("  "), []);
        assert_eq!(clauses("…?"), [("…".to_string(), Some('?'))]);
    }

    #[test]
    fn spells_espeak_ipa_the_kokoro_way() {
        assert_eq!(to_kokoro("həlˈo^ʊ\n"), "həlˈO");
        assert_eq!(to_kokoro("ɛnd^ʒˈɔ^ɪ  ðə\nmjˈuːzɪk"), "ɛnʤˈY ðə mjˈuːzɪk");
        assert_eq!(to_kokoro("bɹˈa^ʊzɚ t^ʃˈa^ɪ jʊ^ɹ"), "bɹˈWzɚ ʧˈI jʊɹ");
    }

    #[test]
    fn unknown_symbols_are_dropped() {
        let vocab = HashMap::from([('a', 43), ('b', 44), ('!', 5)]);
        assert_eq!(tokens("ab!?c", &vocab), [43, 44, 5]);
    }

    #[test]
    fn packs_clauses_and_cuts_long_ones() {
        let c = |n: usize| vec![1_i64; n];
        let lens = |b: Vec<Vec<i64>>| b.iter().map(Vec::len).collect::<Vec<_>>();
        assert_eq!(lens(batches(vec![c(3), c(4), c(5)], 10)), [7, 5]);
        assert_eq!(lens(batches(vec![c(25), c(2)], 10)), [10, 2]);
        assert_eq!(lens(batches(vec![c(0), c(2), c(0)], 10)), [2]);
        assert_eq!(batches(vec![], 10), Vec::<Vec<i64>>::new());
    }

    #[test]
    fn missing_model_is_a_clear_error() {
        let Err(e) = Kokoro::load(Path::new("/nonexistent/kokoro")) else { panic!("loaded nothing") };
        assert!(e.contains("download-model.sh kokoro"), "{e}");
    }

    /// Kokoro speaks, Whisper listens: the words must survive the round trip.
    fn round_trip(text: &str) -> Vec<String> {
        let mut kokoro = Kokoro::load(&model_dir()).unwrap();
        let mut took = Vec::new();
        let mut audio = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            audio = kokoro.speak(text, "af_heart").unwrap();
            took.push(started.elapsed());
        }
        assert!(audio.iter().all(|s| s.is_finite()), "Kokoro produced NaN");
        crate::log(&format!("{:.2} s of speech in {took:?}", audio.len() as f32 / RATE as f32));
        let mut at_16k = Vec::new();
        crate::audio::Resampler::new(RATE).push(&audio, &mut at_16k);
        let model = crate::config::Config::default().model_path();
        let model = std::env::var_os("CLAUDE_NOTCH_TEST_MODEL").map_or(model, PathBuf::from);
        let mut stt = crate::stt::Stt::load(&model, vec!["en".into()]).unwrap();
        let heard = stt.transcribe(&at_16k).unwrap();
        crate::log(&format!("heard {heard:?}"));
        crate::wake::normalize(&heard)
    }

    #[test]
    #[ignore = "needs the Kokoro and speech models and espeak-ng: cargo test -- --ignored"]
    fn speaks_english_end_to_end() {
        let words = round_trip("YouTube is open in your browser. Enjoy the music!");
        for word in ["youtube", "open", "browser", "music"] {
            assert!(words.contains(&word.to_string()), "{word} missing from {words:?}");
        }
    }
}
