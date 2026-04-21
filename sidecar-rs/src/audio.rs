use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Sender;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::sync::{Arc, Mutex};

const TARGET_RATE: u32 = 16_000;
pub const CHUNK_LEN: usize = 2_560; // 160 ms @ 16 kHz

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub struct AudioStream {
    _stream: cpal::Stream,
}

pub fn start_capture(device_name: Option<&str>, tx: Sender<Vec<f32>>) -> Result<AudioStream> {
    let host = cpal::default_host();
    let device = match device_name {
        Some(name) => {
            let devs = host.input_devices()?;
            devs.into_iter()
                .find(|d| d.name().map(|n| n.contains(name)).unwrap_or(false))
                .ok_or_else(|| anyhow!("input device not found: {name}"))?
        }
        None => host.default_input_device().ok_or_else(|| anyhow!("no default input device"))?,
    };
    let name = device.name().unwrap_or_else(|_| "<unknown>".into());

    let default_cfg = device
        .default_input_config()
        .context("default_input_config")?;
    let device_rate = default_cfg.sample_rate().0;
    let channels = default_cfg.channels() as usize;
    let sample_format = default_cfg.sample_format();
    let cfg: cpal::StreamConfig = default_cfg.into();

    eprintln!(
        "[sidecar] capture device={name:?} rate={device_rate} chan={channels} fmt={sample_format:?}"
    );

    // Per-chunk input size for rubato: ~160 ms of native-rate audio
    let native_chunk: usize = (device_rate as usize / 1000) * 160;
    let resampler = SincFixedIn::<f32>::new(
        TARGET_RATE as f64 / device_rate as f64,
        2.0,
        SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        },
        native_chunk,
        1,
    )
    .context("rubato init")?;

    let shared: Arc<Mutex<ResamplerState>> = Arc::new(Mutex::new(ResamplerState {
        resampler,
        mono_in: Vec::with_capacity(native_chunk * 4),
        out_buf: Vec::with_capacity(CHUNK_LEN * 4),
        native_chunk,
    }));

    let err_fn = |e| eprintln!("[sidecar] stream error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let shared = shared.clone();
            let tx = tx.clone();
            device.build_input_stream(
                &cfg,
                move |data: &[f32], _| feed_samples(&shared, &tx, channels, data.iter().copied()),
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let shared = shared.clone();
            let tx = tx.clone();
            device.build_input_stream(
                &cfg,
                move |data: &[i16], _| {
                    feed_samples(
                        &shared,
                        &tx,
                        channels,
                        data.iter().map(|&s| s as f32 / i16::MAX as f32),
                    )
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let shared = shared.clone();
            let tx = tx.clone();
            device.build_input_stream(
                &cfg,
                move |data: &[u16], _| {
                    feed_samples(
                        &shared,
                        &tx,
                        channels,
                        data.iter()
                            .map(|&s| (s as f32 - u16::MAX as f32 / 2.0) / (u16::MAX as f32 / 2.0)),
                    )
                },
                err_fn,
                None,
            )?
        }
        fmt => return Err(anyhow!("unsupported sample format: {fmt:?}")),
    };

    stream.play()?;
    Ok(AudioStream { _stream: stream })
}

struct ResamplerState {
    resampler: SincFixedIn<f32>,
    mono_in: Vec<f32>,
    out_buf: Vec<f32>,
    native_chunk: usize,
}

fn feed_samples<I: Iterator<Item = f32>>(
    shared: &Arc<Mutex<ResamplerState>>,
    tx: &Sender<Vec<f32>>,
    channels: usize,
    samples: I,
) {
    let mut st = match shared.lock() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("[sidecar] lock poisoned: {e}");
            return;
        }
    };

    // Downmix to mono
    if channels <= 1 {
        st.mono_in.extend(samples);
    } else {
        let mut acc = 0.0f32;
        let mut n = 0usize;
        for s in samples {
            acc += s;
            n += 1;
            if n == channels {
                st.mono_in.push(acc / channels as f32);
                acc = 0.0;
                n = 0;
            }
        }
    }

    // Resample in full native_chunk units
    let native_chunk = st.native_chunk;
    while st.mono_in.len() >= native_chunk {
        let take: Vec<f32> = st.mono_in.drain(..native_chunk).collect();
        let input = [take];
        match st.resampler.process(&input, None) {
            Ok(out) => st.out_buf.extend_from_slice(&out[0]),
            Err(e) => {
                eprintln!("[sidecar] resample error: {e}");
                return;
            }
        }
    }

    // Emit fixed-length 16 kHz chunks
    while st.out_buf.len() >= CHUNK_LEN {
        let chunk: Vec<f32> = st.out_buf.drain(..CHUNK_LEN).collect();
        if tx.send(chunk).is_err() {
            break; // receiver dropped
        }
    }
}
