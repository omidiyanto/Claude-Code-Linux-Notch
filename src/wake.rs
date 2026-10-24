//! What was said: the wake phrase with a command, plain speech, or recognition noise.
//! Pure text handling, so every case is a unit test.

/// How Whisper tends to write "Claude", all read as "claude".
const NAME_VARIANTS: &[&str] = &[
    "claude", "cloud", "clod", "klod", "claud", "klaud", "kloud", "clode", "klode", "clot", "klot", "cloth", "clawd",
    "claudes", "clouds", "klaude",
];

/// Up to this many filler words ("oh", "eh", "um") may come before the wake phrase.
const MAX_LEADING_WORDS: usize = 2;

/// What Whisper writes for silence, music or background noise.
const NOISE: &[&str] = &[
    "thank you",
    "thanks",
    "thanks for watching",
    "thank you for watching",
    "you",
    "bye",
    "music",
    "applause",
    "subtitles by the amara org community",
];

/// A whole command that only means "stop what you are doing".
const CANCEL: &[&str] = &["stop", "stop it", "cancel", "never mind", "nevermind"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    /// The wake phrase, then what followed it (possibly empty), as originally written.
    Wake(String),
    Speech(String),
    Noise,
}

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        _ => c,
    }
}

fn canonical(word: &str) -> String {
    let w: String = word.chars().flat_map(char::to_lowercase).map(fold).collect();
    if NAME_VARIANTS.contains(&w.as_str()) { "claude".into() } else { w }
}

/// Words as (byte end in the original text, canonical form).
fn words(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((i, canonical(&text[s..i])));
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// Removes Whisper's annotations: `[BLANK_AUDIO]`, `(music)`, `*laughs*`, `♪`.
/// An opener without its closer is ordinary text ("5*3").
fn strip_annotations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let closer = match c {
            '[' => Some(']'),
            '(' => Some(')'),
            '*' => Some('*'),
            _ => None,
        };
        // Openers are one byte, so `rest[1..]` is on a char boundary.
        if let Some(end) = closer.and_then(|closer| rest[1..].find(closer)) {
            rest = &rest[end + 2..];
            continue;
        }
        if c != '♪' && c != '♫' {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Canonical words of `text`: lower case, accents folded, name variants as "claude".
pub fn normalize(text: &str) -> Vec<String> {
    words(&strip_annotations(text)).into_iter().map(|(_, w)| w).collect()
}

pub fn hear(text: &str, phrases: &[String]) -> Heard {
    let text = strip_annotations(text);
    let words = words(&text);
    let joined = words.iter().map(|(_, w)| w.as_str()).collect::<Vec<_>>().join(" ");
    if joined.is_empty() || NOISE.contains(&joined.as_str()) {
        return Heard::Noise;
    }
    for start in 0..=MAX_LEADING_WORDS.min(words.len()) {
        for phrase in phrases {
            let phrase = normalize(phrase);
            let end = start + phrase.len();
            if phrase.is_empty() || end > words.len() {
                continue;
            }
            if words[start..end].iter().map(|(_, w)| w).eq(phrase.iter()) {
                let rest = &text[words[end - 1].0..];
                let command = rest.trim_start_matches(|c: char| !c.is_alphanumeric()).trim_end();
                return Heard::Wake(command.to_string());
            }
        }
    }
    Heard::Speech(text.trim().to_string())
}

pub fn is_cancel(command: &str) -> bool {
    CANCEL.contains(&normalize(command).join(" ").as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phrases() -> Vec<String> {
        crate::config::Config::default().wake_phrases
    }

    fn wake(s: &str) -> Heard {
        Heard::Wake(s.into())
    }

    #[test]
    fn wake_phrase_alone() {
        assert_eq!(hear("Hi Claude.", &phrases()), wake(""));
        assert_eq!(hear(" hey claude!! ", &phrases()), wake(""));
        assert_eq!(hear("Hey, Klod?", &phrases()), wake(""));
    }

    #[test]
    fn wake_phrase_with_command_keeps_original_text() {
        // A command after a misheard name.
        assert_eq!(
            hear("Hey Cloud, open Firefox and play YouTube.", &phrases()),
            wake("open Firefox and play YouTube.")
        );
        assert_eq!(
            hear("Hi Claude, let's code: new project todo-api.", &phrases()),
            wake("let's code: new project todo-api.")
        );
        assert_eq!(hear("OK Claude open ~/Projects", &phrases()), wake("open ~/Projects"));
    }

    #[test]
    fn filler_words_before_the_phrase() {
        assert_eq!(hear("Oh, hi Claude, open firefox", &phrases()), wake("open firefox"));
        assert_eq!(hear("Eh, um, hey Claude stop", &phrases()), wake("stop"));
        assert!(matches!(hear("so I said to him hey claude", &phrases()), Heard::Speech(_)));
    }

    #[test]
    fn accents_and_case_fold() {
        assert_eq!(hear("HÉY CLÄUDE", &phrases()), wake(""));
    }

    #[test]
    fn talking_about_claude_is_not_a_wake() {
        assert_eq!(hear("I really like Claude Code.", &phrases()), Heard::Speech("I really like Claude Code.".into()));
        assert!(matches!(hear("claude", &phrases()), Heard::Speech(_)));
    }

    #[test]
    fn recognition_noise() {
        for noise in
            ["", "   ", "[BLANK_AUDIO]", "(music)", "♪ ♪", "Thanks for watching!", "Thank you.", "*applause*", "..."]
        {
            assert_eq!(hear(noise, &phrases()), Heard::Noise, "{noise:?}");
        }
    }

    #[test]
    fn custom_phrases() {
        let custom = vec!["yo buddy".to_string()];
        assert_eq!(hear("Yo, buddy! lights off", &custom), wake("lights off"));
        assert!(matches!(hear("hey claude", &custom), Heard::Speech(_)));
    }

    #[test]
    fn cancel_is_the_whole_command_only() {
        for yes in ["Stop.", "Cancel!", "never mind", "Stop it"] {
            assert!(is_cancel(yes), "{yes}");
        }
        for no in ["stop the music", "cancel tomorrow's meeting", "", "play"] {
            assert!(!is_cancel(no), "{no}");
        }
    }

    #[test]
    fn unclosed_annotation_is_text() {
        assert_eq!(hear("Hey Claude compute 5*3", &phrases()), wake("compute 5*3"));
        assert_eq!(hear("Hi Claude (open firefox", &phrases()), wake("open firefox"));
        assert_eq!(hear("Hi Claude [ok] open (music) firefox", &phrases()), wake("open  firefox"));
    }

    #[test]
    fn normalize_maps_name_variants() {
        assert_eq!(normalize("Hey, CLOUD!"), vec!["hey", "claude"]);
        assert_eq!(normalize("[x] (y)"), Vec::<String>::new());
    }
}
