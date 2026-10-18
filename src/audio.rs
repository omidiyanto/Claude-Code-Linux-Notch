//! Microphone capture: the default input device, downmixed and resampled to 16 kHz mono,
//! then cut into utterances by the segmenter. Nothing is recorded to disk.

use std::sync::mpsc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};

use crate::vad::{self, FRAME, SAMPLE_RATE, Segmenter, VadEvent};

#[derive(Debug)]
pub enum AudioEvent {
    /// Microphone level (RMS), about 16 times a second.
    Level(f32),
    Speech,
    /// The utterance so far, while it is still going.
    Partial(Vec<f32>),
    Utterance(Vec<f32>),
}

/// Converts any input rate to 16 kHz: averages the samples of each output period when
/// downsampling (a cheap anti-alias filter), repeats them when upsampling.
#[derive(Debug)]
pub struct Resampler {
    step: f64,
    t: f64,
    acc: f32,
    n: u32,
}

impl Resampler {
    pub fn new(from_rate: u32) -> Self {
        Self { step: f64::from(from_rate) / SAMPLE_RATE as f64, t: 0.0, acc: 0.0, n: 0 }
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            self.acc += x;
            self.n += 1;
            self.t += 1.0;
            if self.t >= self.step {
                let value = self.acc / self.n as f32;
                self.acc = 0.0;
                self.n = 0;
                while self.t >= self.step {
                    out.push(value);
                    self.t -= self.step;
                }
            }
        }
    }
}

/// Starts the microphone. The returned sender turns it on (`true`) and off (`false`): off closes
/// the device, so the desktop's microphone indicator goes away too. Dropping the sender stops it.
pub fn spawn(
    min_threshold: f32,
    mut sink: impl FnMut(AudioEvent) + Send + 'static,
) -> Result<mpsc::Sender<bool>, String> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (ctl_tx, ctl_rx) = mpsc::channel::<bool>();
    let (pcm_tx, pcm_rx) = mpsc::sync_channel::<Vec<f32>>(64);
    std::thread::Builder::new()
        .name("mic".into())
        .spawn(move || {
            let mut stream = match open(pcm_tx.clone()) {
                Ok(stream) => Some(stream),
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            // cpal streams cannot move between threads, so this one owns it until the sender drops.
            for on in ctl_rx {
                if !on {
                    stream = None;
                } else if stream.is_none() {
                    stream = open(pcm_tx.clone()).map_err(|e| crate::log(&e)).ok();
                }
            }
        })
        .map_err(|e| e.to_string())?;
    ready_rx.recv().map_err(|_| "the microphone thread stopped".to_string())??;

    std::thread::Builder::new()
        .name("vad".into())
        .spawn(move || {
            let mut segmenter = Segmenter::new(min_threshold);
            let mut frame = Vec::with_capacity(FRAME);
            let mut count = 0u32;
            for chunk in pcm_rx {
                for sample in chunk {
                    frame.push(sample);
                    if frame.len() < FRAME {
                        continue;
                    }
                    count = count.wrapping_add(1);
                    if count.is_multiple_of(2) {
                        sink(AudioEvent::Level(vad::rms(&frame)));
                    }
                    match segmenter.push(&frame) {
                        Some(VadEvent::Started) => sink(AudioEvent::Speech),
                        Some(VadEvent::Partial(samples)) => sink(AudioEvent::Partial(samples)),
                        Some(VadEvent::Utterance(samples)) => sink(AudioEvent::Utterance(samples)),
                        None => {}
                    }
                    frame.clear();
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(ctl_tx)
}

fn open(pcm: mpsc::SyncSender<Vec<f32>>) -> Result<cpal::Stream, String> {
    let device = cpal::default_host().default_input_device().ok_or("No microphone found")?;
    let config = device.default_input_config().map_err(|e| format!("microphone: {e}"))?;
    let channels = usize::from(config.channels()).max(1);
    let rate = config.sample_rate();
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config.into(), channels, rate, pcm),
        SampleFormat::I16 => build::<i16>(&device, config.into(), channels, rate, pcm),
        SampleFormat::I32 => build::<i32>(&device, config.into(), channels, rate, pcm),
        SampleFormat::U16 => build::<u16>(&device, config.into(), channels, rate, pcm),
        other => return Err(format!("unsupported microphone format {other}")),
    }
    .map_err(|e| format!("microphone: {e}"))?;
    stream.play().map_err(|e| format!("microphone: {e}"))?;
    Ok(stream)
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    rate: u32,
    pcm: mpsc::SyncSender<Vec<f32>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut resampler = Resampler::new(rate);
    let mut mono = Vec::new();
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            mono.clear();
            mono.extend(
                data.chunks(channels).map(|f| f.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / f.len() as f32),
            );
            let mut out = Vec::with_capacity(mono.len());
            resampler.push(&mono, &mut out);
            // Never block the audio thread: if processing falls this far behind, drop audio.
            let _ = pcm.try_send(out);
        },
        |e| crate::log(&format!("microphone: {e}")),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resample(rate: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(rate);
        let mut out = Vec::new();
        // In uneven chunks, as audio callbacks deliver it.
        for chunk in input.chunks(333) {
            r.push(chunk, &mut out);
        }
        out
    }

    #[test]
    fn identity_at_16k() {
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        assert_eq!(resample(16_000, &input), input);
    }

    #[test]
    fn lengths_follow_the_rate() {
        for (rate, expect) in [(48_000, 16_000), (44_100, 16_000), (8_000, 16_000), (22_050, 16_000)] {
            let out = resample(rate, &vec![0.0; rate as usize]);
            assert!(out.len().abs_diff(expect) <= 1, "{rate}: {}", out.len());
        }
    }

    #[test]
    fn keeps_dc_and_tames_frequencies_above_nyquist() {
        assert!(resample(48_000, &vec![0.25; 4800]).iter().all(|s| (s - 0.25).abs() < 1e-6));
        // 15 kHz cannot exist at 16 kHz; averaging three samples must shrink what folds back.
        let tone: Vec<f32> =
            (0..48_000).map(|i| (i as f32 / 48_000.0 * 15_000.0 * std::f32::consts::TAU).sin()).collect();
        assert!(vad::rms(&resample(48_000, &tone)) < 0.25);
    }
}
