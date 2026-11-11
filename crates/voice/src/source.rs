use std::io::{self, Read, Seek, SeekFrom};

use dn_audio::{CaptureReader, FRAME_SAMPLES, FrameInfo};
use songbird::input::core::io::MediaSource;

const FRAME_BYTES: usize = FRAME_SAMPLES * 4;

/// Anything that yields processed 20 ms mono 48 kHz frames without blocking.
pub(crate) trait FrameSource: Send + Sync + 'static {
    fn read_frame(&mut self, out: &mut [f32; FRAME_SAMPLES]) -> Option<FrameInfo>;
    fn available_frames(&self) -> usize;
}

impl FrameSource for CaptureReader {
    fn read_frame(&mut self, out: &mut [f32; FRAME_SAMPLES]) -> Option<FrameInfo> {
        CaptureReader::read_frame(self, out)
    }

    fn available_frames(&self) -> usize {
        CaptureReader::available_frames(self)
    }
}

/// Endless, non-blocking byte stream of little-endian f32 microphone samples for songbird's
/// raw input format. Returns a silent frame whenever capture has nothing ready, so the track
/// never stalls or ends.
///
/// Symphonia only reads when its buffer is empty and each `read` yields at most one frame, so
/// no audio is buffered ahead of the mixer. Latency is bounded here instead: gate-closed frames
/// that queued up while the track was paused are skipped, and the backlog is capped.
pub(crate) struct CaptureSource<S> {
    source: S,
    frame: [f32; FRAME_SAMPLES],
    bytes: Box<[u8; FRAME_BYTES]>,
    pos: usize,
}

impl<S: FrameSource> CaptureSource<S> {
    /// Frames kept queued at most (80 ms); older ones are dropped even if they carry speech.
    pub const MAX_BACKLOG: usize = 4;

    pub fn new(source: S) -> Self {
        Self { source, frame: [0.0; FRAME_SAMPLES], bytes: Box::new([0; FRAME_BYTES]), pos: FRAME_BYTES }
    }

    fn refill(&mut self) {
        loop {
            let Some(info) = self.source.read_frame(&mut self.frame) else {
                self.frame.fill(0.0);
                break;
            };
            let remaining = self.source.available_frames();
            let stale_silence = !info.voice_active && remaining > 0;
            if stale_silence || remaining >= Self::MAX_BACKLOG {
                continue;
            }
            break;
        }
        for (dst, s) in self.bytes.as_chunks_mut::<4>().0.iter_mut().zip(self.frame.iter()) {
            *dst = s.to_le_bytes();
        }
        self.pos = 0;
    }
}

impl<S: FrameSource> Read for CaptureSource<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.pos >= FRAME_BYTES {
            self.refill();
        }
        let n = buf.len().min(FRAME_BYTES - self.pos);
        buf[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

impl<S: FrameSource> Seek for CaptureSource<S> {
    fn seek(&mut self, _pos: SeekFrom) -> io::Result<u64> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

impl<S: FrameSource> MediaSource for CaptureSource<S> {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;

    use songbird::input::codecs::{get_codec_registry, get_probe};
    use songbird::input::{Input, LiveInput, RawAdapter};

    use super::*;

    #[derive(Default)]
    pub struct QueueSource(pub VecDeque<([f32; FRAME_SAMPLES], bool)>);

    impl QueueSource {
        pub fn push(&mut self, value: f32, active: bool) {
            let mut frame = [0.0; FRAME_SAMPLES];
            for (i, s) in frame.iter_mut().enumerate() {
                *s = value + i as f32 * 1e-4;
            }
            self.0.push_back((frame, active));
        }
    }

    impl FrameSource for QueueSource {
        fn read_frame(&mut self, out: &mut [f32; FRAME_SAMPLES]) -> Option<FrameInfo> {
            let (frame, active) = self.0.pop_front()?;
            *out = frame;
            Some(FrameInfo { voice_active: active, level_db: -20.0 })
        }

        fn available_frames(&self) -> usize {
            self.0.len()
        }
    }

    fn read_samples<R: Read>(reader: &mut R, samples: usize, chunk: usize) -> Vec<f32> {
        let mut bytes = Vec::new();
        let mut buf = vec![0u8; chunk];
        while bytes.len() < samples * 4 {
            let n = reader.read(&mut buf).unwrap();
            assert!(n > 0, "source must never signal EOF");
            bytes.extend_from_slice(&buf[..n]);
        }
        bytes.truncate(samples * 4);
        bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    }

    #[test]
    fn emits_frames_as_le_f32_across_odd_reads() {
        let mut q = QueueSource::default();
        q.push(0.25, true);
        q.push(0.5, true);
        let expected: Vec<f32> = q.0.iter().flat_map(|(f, _)| f.iter().copied()).collect();
        let mut src = CaptureSource::new(q);
        assert_eq!(read_samples(&mut src, FRAME_SAMPLES * 2, 1000), expected);
    }

    #[test]
    fn zero_fills_on_underrun_and_never_ends() {
        let mut src = CaptureSource::new(QueueSource::default());
        let mut buf = [0xAAu8; 4096];
        for _ in 0..100 {
            let n = src.read(&mut buf).unwrap();
            assert!(n > 0 && n <= FRAME_BYTES);
            assert!(buf[..n].iter().all(|&b| b == 0));
        }
        assert!(!src.is_seekable());
        assert!(src.byte_len().is_none());
        assert!(src.seek(SeekFrom::Start(0)).is_err());
    }

    #[test]
    fn skips_queued_silence_but_keeps_speech_onset() {
        let mut q = QueueSource::default();
        for _ in 0..6 {
            q.push(0.0, false);
        }
        q.push(0.7, true);
        q.push(0.8, true);
        let mut src = CaptureSource::new(q);
        let samples = read_samples(&mut src, FRAME_SAMPLES, FRAME_BYTES);
        assert!((samples[0] - 0.7).abs() < 1e-6, "onset frame must be sent first");
        assert_eq!(src.source.available_frames(), 1);
    }

    #[test]
    fn caps_backlog_of_speech() {
        let mut q = QueueSource::default();
        for i in 0..8 {
            q.push(i as f32, true);
        }
        let mut src = CaptureSource::new(q);
        let samples = read_samples(&mut src, FRAME_SAMPLES, FRAME_BYTES);
        assert_eq!(samples[0], 4.0);
        assert_eq!(src.source.available_frames(), CaptureSource::<QueueSource>::MAX_BACKLOG - 1);
    }

    #[tokio::test]
    async fn songbird_parses_the_stream_as_raw_mono_48k() {
        let mut q = QueueSource::default();
        q.push(0.125, true);
        let expected = q.0[0].0;
        let input: Input = RawAdapter::new(CaptureSource::new(q), 48_000, 1).into();
        assert!(matches!(input, Input::Live(LiveInput::Raw(_), _)));
        let mut input = input.make_playable_async(get_codec_registry(), get_probe()).await.unwrap();
        let Some(parsed) = input.parsed_mut() else { panic!("input is not parsed") };
        let params = &parsed.format.default_track().unwrap().codec_params;
        assert_eq!(params.sample_rate, Some(48_000));
        assert_eq!(params.channels.map(|c| c.count()), Some(1));

        let mut samples = Vec::new();
        while samples.len() < FRAME_SAMPLES {
            let packet = parsed.format.next_packet().unwrap();
            samples.extend(packet.buf().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
        }
        assert_eq!(&samples[..FRAME_SAMPLES], &expected[..]);
        // The stream continues with silence instead of ending.
        let packet = parsed.format.next_packet().unwrap();
        assert!(!packet.buf().is_empty());
    }
}
