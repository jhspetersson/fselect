use std::io;
use std::io::Write;

pub struct WritableBuffer {
    buf: String,
}

impl WritableBuffer {
    pub fn new() -> WritableBuffer {
        WritableBuffer { buf: String::new() }
    }
}

impl From<WritableBuffer> for String {
    fn from(wb: WritableBuffer) -> Self {
        wb.buf
    }
}

impl Write for WritableBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use std::fmt::Write;
        match String::from_utf8(buf.into()) {
            Ok(string) => {
                let l = string.len();
                match self.buf.write_str(string.as_str()) {
                    Ok(()) => Ok(l),
                    Err(_) => Err(io::ErrorKind::InvalidInput.into()),
                }
            }
            Err(_) => Err(io::ErrorKind::InvalidInput.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
