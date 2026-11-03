//! Microphone narration: capture with cpal, downmix to mono, resample to
//! 48 kHz and encode 20 ms Opus packets into the shared `screen.mkv`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, SizedSample};
use crossbeam_channel::{Sender, bounded, unbounded};
use openscrib_core::Clock;
use openscrib_core::mkv::{AudioSpec, OPUS_SAMPLE_RATE, Track};
use tracing::{info, warn};

use crate::media::Muxer;

const FRAME: usize = OPUS_SAMPLE_RATE as usize / 50; // 20 ms
const BITRATE: i32 = 32_000;

#[derive(Debug, Clone, Default)]
pub struct AudioConfig {
    /// Substring of the input device name; `None` uses the default device.
    pub device: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AudioStats {
    pub device: String,
    pub device_rate: u32,
    pub first_ms: Option<u64>,
    pub packets: u64,
}

pub struct Narration {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Result<AudioStats>>,
}

/// Input device names, with whether each is the default.
pub fn input_devices() -> Result<Vec<(String, bool)>> {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .and_then(|d| d.description().ok())
        .map(|d| d.name().to_string());
    Ok(host
        .input_devices()?
        .filter_map(|d| d.description().ok().map(|d| d.name().to_string()))
        .map(|n| {
            let is_default = default.as_deref() == Some(n.as_str());
            (n, is_default)
        })
        .collect())
}

/// The track description, known before capture starts so the muxer can
/// write its header as soon as video is ready.
pub fn spec() -> AudioSpec {
    AudioSpec { channels: 1 }
}

impl Narration {
    pub fn start(cfg: &AudioConfig, clock: Clock, muxer: Muxer) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = bounded::<Result<()>>(1);
        let cfg = cfg.clone();
        let flag = stop.clone();
        let handle = thread::Builder::new()
            .name("narration".into())
            .spawn(move || run(cfg, clock, muxer, flag, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop, handle }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err(handle.join().map_err(|_| anyhow!("audio thread panicked"))?.err().unwrap_or_else(|| anyhow!("audio thread exited"))),
        }
    }

    pub fn stop(self) -> Result<AudioStats> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.join().map_err(|_| anyhow!("audio thread panicked"))?
    }
}

struct Chunk {
    t_ms: u64,
    samples: Vec<f32>,
}

fn run(
    cfg: AudioConfig,
    clock: Clock,
    muxer: Muxer,
    stop: Arc<AtomicBool>,
    ready: Sender<Result<()>>,
) -> Result<AudioStats> {
    // Everything that can fail at startup reports through `ready`.
    let setup = (|| -> Result<_> {
        let host = cpal::default_host();
        let device = match &cfg.device {
            None => host.default_input_device().context("no default microphone")?,
            Some(want) => host
                .input_devices()?
                .find(|d| d.description().is_ok_and(|n| n.name().to_lowercase().contains(&want.to_lowercase())))
                .with_context(|| format!("no input device matching \"{want}\""))?,
        };
        let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "unknown".into());
        let supported = device.default_input_config().context("reading microphone config")?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let (tx, rx) = unbounded::<Chunk>();
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, channels, clock, tx)?,
            SampleFormat::I16 => build::<i16>(&device, &config, channels, clock, tx)?,
            SampleFormat::I32 => build::<i32>(&device, &config, channels, clock, tx)?,
            SampleFormat::U16 => build::<u16>(&device, &config, channels, clock, tx)?,
            other => bail!("unsupported microphone sample format {other:?}"),
        };
        let mut enc = opus::Encoder::new(OPUS_SAMPLE_RATE, opus::Channels::Mono, opus::Application::Voip)?;
        enc.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
        stream.play().context("starting microphone")?;
        info!("audio: \"{name}\" {rate} Hz x{channels}");
        Ok((stream, rx, enc, name, rate))
    })();

    let (stream, rx, mut enc, device, device_rate) = match setup {
        Ok(s) => {
            let _ = ready.send(Ok(()));
            s
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = ready.send(Err(e));
            bail!(msg);
        }
    };

    let mut resampler = Resampler::new(device_rate);
    let mut pcm: Vec<f32> = Vec::with_capacity(FRAME * 4);
    let mut out = vec![0u8; 4000];
    let mut first_ms = None;
    let mut packets = 0u64;

    let mut handle_chunk = |chunk: Chunk, pcm: &mut Vec<f32>| -> Result<()> {
        if first_ms.is_none() {
            // The callback fires when the buffer is full, so its first
            // sample was captured one buffer earlier.
            let span = chunk.samples.len() as u64 * 1000 / device_rate as u64;
            first_ms = Some(chunk.t_ms.saturating_sub(span));
        }
        resampler.process(&chunk.samples, pcm);
        let mut offset = 0;
        while pcm.len() - offset >= FRAME {
            let n = enc.encode_float(&pcm[offset..offset + FRAME], &mut out)?;
            let t = first_ms.unwrap_or(0) + packets * 20;
            muxer.push(Track::Audio, t, true, out[..n].to_vec());
            packets += 1;
            offset += FRAME;
        }
        pcm.drain(..offset);
        Ok(())
    };

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => handle_chunk(chunk, &mut pcm)?,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                warn!("microphone stream ended");
                break;
            }
        }
    }
    drop(stream);
    while let Ok(chunk) = rx.try_recv() {
        handle_chunk(chunk, &mut pcm)?;
    }

    Ok(AudioStats { device, device_rate, first_ms, packets })
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    clock: Clock,
    tx: Sender<Chunk>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let t_ms = clock.now_ms();
            let samples = data
                .chunks(channels.max(1))
                .map(|frame| frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / frame.len() as f32)
                .collect();
            let _ = tx.send(Chunk { t_ms, samples });
        },
        |e| warn!("microphone error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// Linear-interpolating resampler to 48 kHz. Plenty for speech that will be
/// transcribed and listened to, not mastered.
struct Resampler {
    step: f64,
    pos: f64,
    buf: Vec<f32>,
}

impl Resampler {
    fn new(in_rate: u32) -> Self {
        Self { step: in_rate as f64 / OPUS_SAMPLE_RATE as f64, pos: 0.0, buf: Vec::new() }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.step == 1.0 {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        loop {
            let i = self.pos as usize;
            if i + 1 >= self.buf.len() {
                break;
            }
            let frac = (self.pos - i as f64) as f32;
            out.push(self.buf[i] * (1.0 - frac) + self.buf[i + 1] * frac);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.pos -= used as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_44k1_to_48k() {
        let mut r = Resampler::new(44_100);
        let mut out = Vec::new();
        // One second in uneven chunks, as a device would deliver it.
        let input: Vec<f32> = (0..44_100).map(|i| (i as f32 / 100.0).sin()).collect();
        for chunk in input.chunks(441 + 7) {
            r.process(chunk, &mut out);
        }
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{} samples", out.len());
        // Interpolated values stay on the curve.
        let t = 1000.0 * 44_100.0 / 48_000.0;
        assert!((out[1000] - (t as f32 / 100.0).sin()).abs() < 0.01);
    }

    #[test]
    fn passthrough_at_48k() {
        let mut r = Resampler::new(48_000);
        let mut out = Vec::new();
        r.process(&[0.1, 0.2, 0.3], &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3]);
    }
}
