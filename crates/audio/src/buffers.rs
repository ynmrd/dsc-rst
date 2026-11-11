use std::collections::VecDeque;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, TrySendError};
use parking_lot::Mutex;

use crate::{AudioEvent, FRAME_SAMPLES, SAMPLE_RATE};

/// Metadata attached to every processed capture frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameInfo {
    /// Gate state after VAD / push-to-talk / mute. Closed frames carry silence.
    pub voice_active: bool,
    /// Post-gain RMS level in dBFS.
    pub level_db: f32,
}

pub(crate) struct CaptureFrame {
    samples: [f32; FRAME_SAMPLES],
    info: FrameInfo,
}

/// Bounded lock-free frame queue; the producer evicts the oldest frame when full.
pub(crate) struct FrameQueue {
    tx: Sender<CaptureFrame>,
    rx: Receiver<CaptureFrame>,
}

impl FrameQueue {
    /// 200 ms of 20 ms frames.
    pub const CAPACITY: usize = 10;

    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::bounded(Self::CAPACITY);
        Self { tx, rx }
    }

    pub fn push(&self, samples: &[f32; FRAME_SAMPLES], info: FrameInfo) {
        let mut frame = CaptureFrame {
            samples: *samples,
            info,
        };
        for _ in 0..2 {
            match self.tx.try_send(frame) {
                Ok(()) => return,
                Err(TrySendError::Full(f)) => {
                    drop(self.rx.try_recv());
                    frame = f;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    pub fn clear(&self) {
        while self.rx.try_recv().is_ok() {}
    }

    pub fn reader(&self) -> CaptureReader {
        CaptureReader {
            rx: self.rx.clone(),
        }
    }
}

/// Consumer of processed 20 ms mono 48 kHz capture frames.
pub struct CaptureReader {
    rx: Receiver<CaptureFrame>,
}

impl CaptureReader {
    /// Copies the oldest queued frame into `out`; `None` when no frame is ready (never blocks).
    pub fn read_frame(&mut self, out: &mut [f32; FRAME_SAMPLES]) -> Option<FrameInfo> {
        let frame = self.rx.try_recv().ok()?;
        out.copy_from_slice(&frame.samples);
        Some(frame.info)
    }

    pub fn available_frames(&self) -> usize {
        self.rx.len()
    }

    pub fn clear(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }
}

/// Interleaved sample FIFO that drops the oldest samples beyond `max_len` and, on the read
/// side, waits for `prebuffer_len` samples after every underrun before resuming output.
pub(crate) struct JitterBuffer {
    buf: VecDeque<f32>,
    max_len: usize,
    prebuffer_len: usize,
    primed: bool,
}

impl JitterBuffer {
    pub fn new(max_len: usize, prebuffer_len: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(max_len + 2 * FRAME_SAMPLES * 2),
            max_len,
            prebuffer_len: prebuffer_len.min(max_len),
            primed: false,
        }
    }

    /// Appends samples, evicting the oldest beyond capacity. Returns how many were kept.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let samples = &samples[samples.len().saturating_sub(self.max_len)..];
        let overflow = (self.buf.len() + samples.len()).saturating_sub(self.max_len);
        self.buf.drain(..overflow);
        self.buf.extend(samples.iter().copied());
        samples.len()
    }

    /// Fills `out` from the buffer, zero-filling whatever is unavailable. Returns samples read.
    pub fn pop_into(&mut self, out: &mut [f32]) -> usize {
        if !self.primed && self.buf.len() >= self.prebuffer_len.max(1) {
            self.primed = true;
        }
        let n = if self.primed {
            out.len().min(self.buf.len())
        } else {
            0
        };
        for (dst, src) in out.iter_mut().zip(self.buf.drain(..n)) {
            *dst = src;
        }
        out[n..].fill(0.0);
        if n < out.len() {
            self.primed = false;
        }
        n
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.primed = false;
    }
}

pub(crate) type SharedJitter = Arc<Mutex<JitterBuffer>>;

/// Playback queue: stereo, capped at 120 ms, 40 ms prebuffer.
pub(crate) fn playback_buffer() -> SharedJitter {
    Arc::new(Mutex::new(JitterBuffer::new(
        ms_to_samples(120, 2),
        ms_to_samples(40, 2),
    )))
}

/// Mic-test monitor queue: mono, capped at 100 ms, 40 ms prebuffer.
pub(crate) fn monitor_buffer() -> SharedJitter {
    Arc::new(Mutex::new(JitterBuffer::new(
        ms_to_samples(100, 1),
        ms_to_samples(40, 1),
    )))
}

pub(crate) const fn ms_to_samples(ms: usize, channels: usize) -> usize {
    SAMPLE_RATE as usize / 1000 * ms * channels
}

/// Producer of stereo interleaved 48 kHz f32 audio for the output device.
#[derive(Clone)]
pub struct PlaybackWriter {
    buf: SharedJitter,
}

impl PlaybackWriter {
    pub(crate) fn new(buf: SharedJitter) -> Self {
        Self { buf }
    }

    /// Queues samples (a trailing odd sample is ignored). When more than ~120 ms would be
    /// buffered the oldest audio is dropped so latency cannot grow. Returns samples queued.
    pub fn write(&mut self, stereo: &[f32]) -> usize {
        let stereo = &stereo[..stereo.len() & !1];
        self.buf.lock().push(stereo)
    }

    pub fn buffered_ms(&self) -> f32 {
        self.buf.lock().len() as f32 / 2.0 / (SAMPLE_RATE as f32 / 1000.0)
    }
}

/// Bounded event channel that discards the oldest event instead of blocking or growing.
pub(crate) struct EventSink {
    tx: Sender<AudioEvent>,
    rx: Receiver<AudioEvent>,
}

impl EventSink {
    const CAPACITY: usize = 256;

    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::bounded(Self::CAPACITY);
        Self { tx, rx }
    }

    pub fn emit(&self, event: AudioEvent) {
        let mut event = event;
        for _ in 0..2 {
            match self.tx.try_send(event) {
                Ok(()) => return,
                Err(TrySendError::Full(e)) => {
                    drop(self.rx.try_recv());
                    event = e;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    pub fn subscribe(&self) -> Receiver<AudioEvent> {
        self.rx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(active: bool) -> FrameInfo {
        FrameInfo {
            voice_active: active,
            level_db: -30.0,
        }
    }

    #[test]
    fn frame_queue_is_bounded_and_drops_oldest() {
        let q = FrameQueue::new();
        let mut reader = q.reader();
        for i in 0..25 {
            q.push(&[i as f32; FRAME_SAMPLES], info(i % 2 == 0));
        }
        assert_eq!(reader.available_frames(), FrameQueue::CAPACITY);
        let mut out = [0.0; FRAME_SAMPLES];
        let first = reader.read_frame(&mut out).unwrap();
        assert_eq!(out[0], 15.0, "oldest surviving frame is #15");
        assert!(!first.voice_active);
        assert_eq!(reader.available_frames(), FrameQueue::CAPACITY - 1);
        reader.clear();
        assert!(reader.read_frame(&mut out).is_none());
    }

    #[test]
    fn jitter_buffer_caps_latency_and_primes() {
        let mut jb = JitterBuffer::new(100, 40);
        assert_eq!(jb.push(&[1.0; 30]), 30);
        let mut out = [9.0; 10];
        assert_eq!(jb.pop_into(&mut out), 0, "not primed yet");
        assert!(out.iter().all(|&s| s == 0.0));

        jb.push(&(0..150).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(jb.len(), 100);
        assert_eq!(jb.pop_into(&mut out), 10);
        assert_eq!(out[0], 50.0, "oldest samples were dropped");

        let mut big = [7.0; 120];
        assert_eq!(jb.pop_into(&mut big), 90);
        assert!(
            big[90..].iter().all(|&s| s == 0.0),
            "underrun is zero-filled"
        );
        jb.push(&[1.0; 20]);
        assert_eq!(jb.pop_into(&mut out), 0, "re-primes after an underrun");
    }

    #[test]
    fn playback_writer_bounds_buffered_time() {
        let buf = playback_buffer();
        let mut writer = PlaybackWriter::new(buf.clone());
        let chunk = [0.1f32; 1920];
        for _ in 0..20 {
            writer.write(&chunk);
        }
        assert!(
            (writer.buffered_ms() - 120.0).abs() < 0.01,
            "{}",
            writer.buffered_ms()
        );
        assert_eq!(writer.write(&[0.5; 3]), 2, "odd trailing sample ignored");
    }

    #[test]
    fn event_sink_drops_oldest() {
        let sink = EventSink::new();
        let rx = sink.subscribe();
        for i in 0..300 {
            sink.emit(AudioEvent::Error(i.to_string()));
        }
        assert_eq!(rx.len(), EventSink::CAPACITY);
        match rx.try_recv().unwrap() {
            AudioEvent::Error(s) => assert_eq!(s, "44"),
            other => panic!("unexpected {other:?}"),
        }
    }
}
