//! Cuts 16 kHz mono audio into utterances with an energy detector whose threshold follows
//! the room's noise floor. Only these utterances ever reach speech recognition.

use std::collections::VecDeque;

pub const SAMPLE_RATE: usize = 16_000;
/// 30 ms.
pub const FRAME: usize = 480;

/// 90 ms of loud frames opens an utterance.
const START_FRAMES: usize = 3;
/// 1.2 s of quiet closes it.
const END_FRAMES: usize = 40;
/// 300 ms from before the start is kept, so the first syllable is not clipped.
const PREROLL: usize = 10 * FRAME;
/// Under 240 ms of loud frames it was a click or a cough.
const MIN_VOICED_FRAMES: usize = 8;
/// 30 s hard cap.
const MAX_FRAMES: usize = 1000;
/// While someone speaks, a snapshot of the utterance so far about every second, for live captions.
const PARTIAL_FRAMES: usize = 33;
/// No snapshot once speech has paused this long: the end is likely near, and recognition
/// should be free for the final pass.
const PARTIAL_QUIET: usize = 8;
const NOISE_ADAPT: f32 = 0.05;
const NOISE_RATIO: f32 = 2.5;

#[derive(Debug, PartialEq)]
pub enum VadEvent {
    Started,
    /// The utterance so far; it is still going.
    Partial(Vec<f32>),
    Utterance(Vec<f32>),
}

pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

struct Active {
    samples: Vec<f32>,
    frames: usize,
    voiced: usize,
    quiet_run: usize,
    since_partial: usize,
    level_sum: f32,
}

pub struct Segmenter {
    min_threshold: f32,
    noise: f32,
    preroll: VecDeque<f32>,
    loud_run: usize,
    active: Option<Active>,
}

impl Segmenter {
    pub fn new(min_threshold: f32) -> Self {
        Self {
            min_threshold,
            noise: min_threshold / NOISE_RATIO,
            preroll: VecDeque::with_capacity(PREROLL + FRAME),
            loud_run: 0,
            active: None,
        }
    }

    pub fn threshold(&self) -> f32 {
        (self.noise * NOISE_RATIO).max(self.min_threshold)
    }

    /// Feeds one frame of exactly [`FRAME`] samples.
    pub fn push(&mut self, frame: &[f32]) -> Option<VadEvent> {
        debug_assert_eq!(frame.len(), FRAME);
        let level = rms(frame);
        let loud = level > self.threshold();

        if let Some(active) = &mut self.active {
            active.samples.extend_from_slice(frame);
            active.frames += 1;
            active.level_sum += level;
            if loud {
                active.quiet_run = 0;
                active.voiced += 1;
            } else {
                active.quiet_run += 1;
            }
            if active.quiet_run < END_FRAMES && active.frames < MAX_FRAMES {
                active.since_partial += 1;
                if active.since_partial < PARTIAL_FRAMES || active.quiet_run >= PARTIAL_QUIET {
                    return None;
                }
                active.since_partial = 0;
                return Some(VadEvent::Partial(active.samples.clone()));
            }
            let done = self.active.take()?;
            if done.frames >= MAX_FRAMES {
                // A sound that never stops (a fan, a vacuum) becomes the new floor instead of
                // feeding speech recognition forever.
                self.noise = done.level_sum / done.frames as f32;
            }
            return (done.voiced >= MIN_VOICED_FRAMES).then_some(VadEvent::Utterance(done.samples));
        }

        self.preroll.extend(frame);
        let excess = self.preroll.len().saturating_sub(PREROLL);
        self.preroll.drain(..excess);
        if loud {
            self.loud_run += 1;
        } else {
            self.loud_run = 0;
            self.noise += (level - self.noise) * NOISE_ADAPT;
        }
        if self.loud_run < START_FRAMES {
            return None;
        }
        let samples: Vec<f32> = self.preroll.drain(..).collect();
        let frames = samples.len() / FRAME;
        self.active = Some(Active {
            samples,
            frames,
            voiced: self.loud_run,
            quiet_run: 0,
            since_partial: 0,
            level_sum: level * frames as f32,
        });
        self.loud_run = 0;
        Some(VadEvent::Started)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: f32, frames: usize) -> Vec<Vec<f32>> {
        (0..frames)
            .map(|f| {
                (0..FRAME)
                    .map(|i| {
                        let t = (f * FRAME + i) as f32 / SAMPLE_RATE as f32;
                        amp * (t * 220.0 * std::f32::consts::TAU).sin()
                    })
                    .collect()
            })
            .collect()
    }

    fn run(seg: &mut Segmenter, frames: &[Vec<f32>]) -> Vec<VadEvent> {
        frames.iter().filter_map(|f| seg.push(f)).collect()
    }

    #[test]
    fn rms_of_known_signals() {
        assert!(rms(&[]).abs() < f32::EPSILON);
        assert!((rms(&[0.5; 8]) - 0.5).abs() < 1e-6);
        let sine = &tone(1.0, 1)[0];
        assert!((rms(sine) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn silence_produces_nothing() {
        let mut seg = Segmenter::new(0.01);
        assert_eq!(run(&mut seg, &tone(0.0, 300)), Vec::new());
    }

    #[test]
    fn one_second_of_speech_is_one_utterance_with_preroll() {
        let mut seg = Segmenter::new(0.01);
        let mut frames = tone(0.0, 20);
        frames.extend(tone(0.3, 33));
        frames.extend(tone(0.0, 60));
        let events: Vec<_> =
            run(&mut seg, &frames).into_iter().filter(|e| !matches!(e, VadEvent::Partial(_))).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], VadEvent::Started);
        let VadEvent::Utterance(samples) = &events[1] else { panic!("no utterance") };
        // 300 ms preroll + ~1 s speech + 1.2 s tail.
        let secs = samples.len() as f32 / SAMPLE_RATE as f32;
        assert!((2.2..=2.6).contains(&secs), "{secs}");
    }

    #[test]
    fn partials_grow_while_speaking_and_stop_in_the_final_pause() {
        let mut seg = Segmenter::new(0.01);
        let mut frames = tone(0.3, 110);
        frames.extend(tone(0.0, 60));
        let events = run(&mut seg, &frames);
        let partials: Vec<usize> = events
            .iter()
            .filter_map(|e| if let VadEvent::Partial(s) = e { Some(s.len() / FRAME) } else { None })
            .collect();
        // ~3.3 s of speech: snapshots at about 1, 2 and 3 s, each longer than the last.
        assert_eq!(partials.len(), 3, "{partials:?}");
        assert!(partials.windows(2).all(|w| w[1] > w[0]), "{partials:?}");
        // None during the closing pause, and the utterance still comes last.
        assert!(partials.iter().all(|&f| f <= 110 + PARTIAL_QUIET), "{partials:?}");
        assert!(matches!(events.last(), Some(VadEvent::Utterance(_))));
    }

    #[test]
    fn a_click_is_dropped() {
        let mut seg = Segmenter::new(0.01);
        let mut frames = tone(0.3, 4);
        frames.extend(tone(0.0, 60));
        assert_eq!(run(&mut seg, &frames), vec![VadEvent::Started]);
    }

    #[test]
    fn endless_sound_is_capped_then_becomes_the_floor() {
        let mut seg = Segmenter::new(0.01);
        let events = run(&mut seg, &tone(0.2, MAX_FRAMES + 200));
        let utterances = events.iter().filter(|e| matches!(e, VadEvent::Utterance(_))).count();
        assert_eq!(utterances, 1);
        assert!(seg.threshold() > 0.2, "{}", seg.threshold());
    }

    #[test]
    fn steady_hum_below_threshold_raises_floor_without_triggering() {
        let mut seg = Segmenter::new(0.01);
        let before = seg.threshold();
        assert_eq!(run(&mut seg, &tone(0.003, 500)), Vec::new());
        assert!(seg.threshold() >= before);
    }
}
