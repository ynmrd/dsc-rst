use flate2::{Decompress, FlushDecompress};

const ZLIB_SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
/// Buffers grown by a huge payload (e.g. GUILD_CREATE of a big guild) are shrunk
/// back to this size so steady-state memory stays small.
const RETAIN_CAPACITY: usize = 64 * 1024;

/// `compress=zlib-stream` transport decoder. One instance per connection: the zlib
/// context spans the whole connection, and a payload may be split across frames.
pub(crate) struct ZlibStream {
    decompress: Decompress,
    input: Vec<u8>,
    output: Vec<u8>,
}

impl ZlibStream {
    pub fn new() -> Self {
        ZlibStream { decompress: Decompress::new(true), input: Vec::new(), output: Vec::new() }
    }

    /// Feeds one binary frame. Returns the decompressed payload once a frame ends
    /// with the `00 00 ff ff` sync-flush marker.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<&[u8]>, String> {
        self.input.extend_from_slice(frame);
        if !self.input.ends_with(&ZLIB_SUFFIX) {
            return Ok(None);
        }
        if self.output.capacity() > RETAIN_CAPACITY {
            self.output = Vec::with_capacity(RETAIN_CAPACITY);
        }
        self.output.clear();
        let mut offset = 0;
        loop {
            if self.output.capacity() - self.output.len() < 1024 {
                let grow = self.output.capacity().max(self.input.len() * 4).max(8 * 1024);
                self.output.reserve(grow);
            }
            let in_before = self.decompress.total_in();
            let out_before = self.output.len();
            self.decompress
                .decompress_vec(&self.input[offset..], &mut self.output, FlushDecompress::Sync)
                .map_err(|e| format!("zlib-stream decompression failed: {e}"))?;
            let consumed = (self.decompress.total_in() - in_before) as usize;
            offset += consumed;
            let produced = self.output.len() - out_before;
            let output_full = self.output.len() == self.output.capacity();
            if offset >= self.input.len() && !output_full {
                break;
            }
            if consumed == 0 && produced == 0 && !output_full {
                return Err("zlib-stream decompression made no progress".into());
            }
        }
        if self.input.capacity() > RETAIN_CAPACITY {
            self.input = Vec::with_capacity(RETAIN_CAPACITY);
        } else {
            self.input.clear();
        }
        Ok(Some(&self.output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compress, Compression, FlushCompress};

    fn compress(c: &mut Compress, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 1024);
        let start = c.total_in();
        loop {
            let consumed = (c.total_in() - start) as usize;
            c.compress_vec(&data[consumed..], &mut out, FlushCompress::Sync).unwrap();
            if (c.total_in() - start) as usize == data.len() && out.len() < out.capacity() {
                break;
            }
            out.reserve(out.capacity().max(1024));
        }
        out
    }

    #[test]
    fn shared_context_and_split_frames() {
        let mut c = Compress::new(Compression::default(), true);
        let mut z = ZlibStream::new();
        let a = compress(&mut c, br#"{"op":10,"d":{"heartbeat_interval":41250}}"#);
        assert_eq!(z.push(&a).unwrap().unwrap(), br#"{"op":10,"d":{"heartbeat_interval":41250}}"#);
        // Second payload depends on the dictionary built by the first.
        let big: String = (0..5000).map(|i| format!("{{\"n\":{i}}},")).collect();
        let b = compress(&mut c, big.as_bytes());
        let (x, y) = b.split_at(b.len() / 2);
        assert!(z.push(x).unwrap().is_none());
        assert_eq!(z.push(y).unwrap().unwrap(), big.as_bytes());
    }
}
