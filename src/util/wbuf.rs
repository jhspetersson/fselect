use std::io;
use std::io::Write;

#[derive(Debug)]
pub struct WritableBuffer {
    buf: String,
    // Incomplete UTF-8 tail carried between writes: buffered writers (e.g.
    // csv::Writer) flush at arbitrary byte offsets, so a chunk may end in the
    // middle of a multibyte sequence that the next chunk completes.
    pending: Vec<u8>,
}

impl WritableBuffer {
    pub fn new() -> WritableBuffer {
        WritableBuffer {
            buf: String::new(),
            pending: Vec::new(),
        }
    }
}

impl From<WritableBuffer> for String {
    fn from(mut wb: WritableBuffer) -> Self {
        if !wb.pending.is_empty() {
            // A truncated final sequence: surface it as replacement chars
            // rather than silently dropping bytes.
            wb.buf.push_str(&String::from_utf8_lossy(&wb.pending));
        }
        wb.buf
    }
}

impl Write for WritableBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut owned: Vec<u8>;
        let bytes: &[u8] = if self.pending.is_empty() {
            buf
        } else {
            owned = std::mem::take(&mut self.pending);
            owned.extend_from_slice(buf);
            &owned
        };

        match std::str::from_utf8(bytes) {
            Ok(s) => {
                self.buf.push_str(s);
            }
            Err(err) => {
                if err.error_len().is_some() {
                    // Genuinely invalid bytes, not a sequence split by
                    // chunking.
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                let valid_up_to = err.valid_up_to();
                let valid = std::str::from_utf8(&bytes[..valid_up_to])
                    .expect("prefix up to valid_up_to is valid UTF-8");
                self.buf.push_str(valid);
                self.pending = bytes[valid_up_to..].to_vec();
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_utf8_and_appends() {
        let mut wb = WritableBuffer::new();
        assert_eq!(wb.write(b"hello").unwrap(), 5);
        assert_eq!(wb.write(b" world").unwrap(), 6);
        assert_eq!(String::from(wb), "hello world");
    }

    #[test]
    fn writes_multibyte_utf8() {
        let mut wb = WritableBuffer::new();
        let s = "héllo 🌍".as_bytes();
        assert_eq!(wb.write(s).unwrap(), s.len());
        assert_eq!(String::from(wb), "héllo 🌍");
    }

    #[test]
    fn rejects_invalid_utf8() {
        let mut wb = WritableBuffer::new();
        let err = wb.write(&[0xff, 0xfe, 0xfd]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn accepts_multibyte_sequence_split_across_writes() {
        // "é" is [0xC3, 0xA9]; a buffered writer may split it anywhere.
        let mut wb = WritableBuffer::new();
        assert_eq!(wb.write(&[b'a', 0xC3]).unwrap(), 2);
        assert_eq!(wb.write(&[0xA9, b'b']).unwrap(), 2);
        assert_eq!(String::from(wb), "aéb");
    }

    #[test]
    fn accepts_four_byte_sequence_split_byte_by_byte() {
        let mut wb = WritableBuffer::new();
        for &b in "🌍".as_bytes() {
            assert_eq!(wb.write(&[b]).unwrap(), 1);
        }
        assert_eq!(String::from(wb), "🌍");
    }

    #[test]
    fn rejects_invalid_continuation_after_split() {
        let mut wb = WritableBuffer::new();
        assert_eq!(wb.write(&[0xC3]).unwrap(), 1);
        let err = wb.write(&[b'x']).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
