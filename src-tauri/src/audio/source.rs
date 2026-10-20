//! The decoding side of the audio engine: what a source is and how one is opened.

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use anyhow::anyhow;

use super::ffmpeg::FfmpegSource;
use super::symph::SymphoniaSource;

/// Sample rate and channel count of an interleaved f32 stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Where audio comes from. M3 adds `Url`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    File(PathBuf),
}

/// A decoded audio stream.
pub trait AudioSource: Send {
    fn spec(&self) -> StreamSpec;
    /// Appends interleaved f32 samples to `out` and returns the frames read; 0 = end of stream.
    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<usize>;
    fn seek(&mut self, ms: u64) -> anyhow::Result<()>;
    fn duration_ms(&self) -> Option<u64>;
}

/// FFmpeg's output spec when the device spec is not known at open time.
pub const FFMPEG_DEFAULT_SPEC: StreamSpec = StreamSpec {
    sample_rate: 48_000,
    channels: 2,
};

/// Opens `input` with symphonia, falling back to FFmpeg (48 kHz stereo) when the probe fails
/// or symphonia has no decoder for the codec.
pub fn open_source(input: &Input) -> anyhow::Result<Box<dyn AudioSource>> {
    open_source_with(input, None)
}

/// Like [`open_source`], with the output device spec (when known) as the FFmpeg target rate.
/// FFmpeg always delivers stereo; symphonia sources keep their native spec.
pub fn open_source_with(
    input: &Input,
    device: Option<StreamSpec>,
) -> anyhow::Result<Box<dyn AudioSource>> {
    if let Some(source) = SymphoniaSource::open_if_supported(input)? {
        return Ok(Box::new(source));
    }
    let target = StreamSpec {
        sample_rate: device.map_or(FFMPEG_DEFAULT_SPEC.sample_rate, |spec| spec.sample_rate),
        channels: 2,
    };
    Ok(Box::new(FfmpegSource::open(input, target)?))
}

/// Runs decoder code, turning a panic into an error so a hostile file cannot kill the
/// engine thread.
pub(crate) fn guarded<T>(what: &str, run: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    match panic::catch_unwind(AssertUnwindSafe(run)) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(anyhow!("{what} panicked: {message}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::tags::fixtures;
    use std::time::{Duration, Instant};

    fn input(name: &str) -> Input {
        Input::File(fixtures().join(name))
    }

    /// Reads a source to the end and returns the number of frames.
    fn drain(source: &mut dyn AudioSource) -> usize {
        let channels = source.spec().channels as usize;
        let mut buf = Vec::new();
        let mut frames = 0;
        loop {
            buf.clear();
            let read = source.read(&mut buf).unwrap();
            if read == 0 {
                return frames;
            }
            assert_eq!(
                buf.len(),
                read * channels,
                "read returns frames, not samples"
            );
            frames += read;
        }
    }

    fn assert_about_one_second(name: &str, source: &mut dyn AudioSource) {
        let rate = source.spec().sample_rate as f64;
        let frames = drain(source) as f64;
        assert!(
            (frames - rate).abs() <= rate * 0.05,
            "{name}: {frames} frames at {rate} Hz is not about 1 s"
        );
    }

    #[test]
    fn symphonia_decodes_common_formats() {
        for name in [
            "tagged.mp3",
            "tagged.flac",
            "notags.ogg",
            "aac.m4a",
            "gap_a.wav",
        ] {
            let mut source = crate::audio::symph::SymphoniaSource::open(&input(name))
                .unwrap_or_else(|err| panic!("{name}: {err:#}"));
            assert_about_one_second(name, &mut source);
            let mut source = open_source(&input(name)).unwrap();
            assert_about_one_second(name, source.as_mut());
        }
    }

    #[test]
    fn ffmpeg_fallback_decodes_rest() {
        if crate::tools::ffmpeg_path().is_none() {
            println!("ffmpeg not found: skipping ffmpeg_fallback_decodes_rest");
            return;
        }
        for name in ["opus.opus", "wma.wma", "wv.wv", "tta.tta"] {
            let mut source =
                open_source(&input(name)).unwrap_or_else(|err| panic!("{name}: {err:#}"));
            assert_eq!(
                source.spec(),
                StreamSpec {
                    sample_rate: 48_000,
                    channels: 2
                },
                "{name}: FFmpeg output defaults to 48 kHz stereo"
            );
            assert_about_one_second(name, source.as_mut());
        }
    }

    #[test]
    fn ffmpeg_uses_the_device_rate_when_known() {
        if crate::tools::ffmpeg_path().is_none() {
            println!("ffmpeg not found: skipping ffmpeg_uses_the_device_rate_when_known");
            return;
        }
        let device = StreamSpec {
            sample_rate: 44_100,
            channels: 2,
        };
        let mut source = open_source_with(&input("opus.opus"), Some(device)).unwrap();
        assert_eq!(source.spec(), device);
        assert_about_one_second("opus.opus", source.as_mut());
    }

    #[test]
    fn seek_lands_near_target() {
        for name in ["tagged.flac", "opus.opus"] {
            if name == "opus.opus" && crate::tools::ffmpeg_path().is_none() {
                continue;
            }
            let mut source = open_source(&input(name)).unwrap();
            let rate = source.spec().sample_rate as f64;
            source.seek(500).unwrap();
            let remaining = drain(source.as_mut()) as f64;
            assert!(
                (remaining - rate * 0.5).abs() <= rate * 0.05,
                "{name}: {remaining} frames left after seeking to 500 ms at {rate} Hz"
            );
        }
    }

    #[test]
    fn corrupt_file_errors_cleanly() {
        let started = Instant::now();
        let result = open_source(&input("corrupt.flac")).and_then(|mut source| {
            let mut buf = Vec::new();
            source.read(&mut buf).map(|_| ())
        });
        assert!(
            result.is_err(),
            "corrupt.flac must fail on open or first read"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn duration_is_known_for_symphonia_sources() {
        let source = open_source(&input("tagged.flac")).unwrap();
        let ms = source.duration_ms().expect("flac has a duration");
        assert!((950..=1050).contains(&ms), "{ms} ms");
    }
}
