use std::fmt;

/// Error type for [`ByteBuffer`] operations.
///
/// Mirrors the `std::runtime_error` cases thrown by the C++ `ByteBuffer`:
/// buffer underflow, oversized UTF strings, negative lengths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByteBufferError {
    Underflow { needed: usize, remaining: usize },
    StringTooLong(usize),
    NegativeLength(i32),
    LengthExceedsMaximum { len: i32, max: i32 },
    InvalidUtf8,
}

impl fmt::Display for ByteBufferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Underflow { needed, remaining } => {
                write!(f, "Buffer underflow: needed {needed}, remaining {remaining}")
            }
            Self::StringTooLong(len) => write!(f, "String too long for writeUTF: {len} bytes"),
            Self::NegativeLength(len) => write!(f, "Negative UTF length: {len}"),
            Self::LengthExceedsMaximum { len, max } => {
                write!(f, "UTF length {len} exceeds maximum {max}")
            }
            Self::InvalidUtf8 => write!(f, "Invalid UTF-8 in buffer"),
        }
    }
}

impl std::error::Error for ByteBufferError {}

impl From<ByteBufferError> for std::io::Error {
    fn from(e: ByteBufferError) -> Self {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    }
}

/// Big-endian byte buffer for Minecraft protocol and NBT I/O.
///
/// Rust port of C++ `ByteBuffer` (`src/core/ByteBuffer.h`).
/// Backed by a `Vec<u8>` with a read cursor (`read_pos`).
/// All multi-byte integers are big-endian. All methods are 1:1 with C++,
/// with method names converted to `snake_case`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ByteBuffer {
    pub data: Vec<u8>,
    pub read_pos: usize,
}

impl ByteBuffer {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            read_pos: 0,
        }
    }

    pub fn from_vec(data: Vec<u8>) -> Self {
        Self { data, read_pos: 0 }
    }

    pub fn with_data_and_pos(data: Vec<u8>, read_pos: usize) -> Self {
        let clamped = if read_pos <= data.len() {
            read_pos
        } else {
            data.len()
        };
        Self {
            data,
            read_pos: clamped,
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.read_pos)
    }

    pub fn reset_cursor(&mut self) {
        self.read_pos = 0;
    }

    pub fn ensure_readable(&self, n: usize) -> Result<(), ByteBufferError> {
        match self.read_pos.checked_add(n) {
            Some(end) if end <= self.data.len() => Ok(()),
            _ => Err(ByteBufferError::Underflow {
                needed: n,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn write_byte(&mut self, v: i8) {
        self.data.push(v as u8);
    }

    pub fn write_ubyte(&mut self, v: u8) {
        self.data.push(v);
    }

    pub fn write_short(&mut self, v: i16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_int(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_long(&mut self, v: i64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_float(&mut self, v: f32) {
        self.write_int(v.to_bits() as i32);
    }

    pub fn write_double(&mut self, v: f64) {
        self.write_long(v.to_bits() as i64);
    }

    pub fn write_bool(&mut self, v: bool) {
        self.write_byte(if v { 1 } else { 0 });
    }

    pub fn write_utf(&mut self, s: &str) -> Result<(), ByteBufferError> {
        if s.len() > 32767 {
            return Err(ByteBufferError::StringTooLong(s.len()));
        }
        let len = s.len() as i16;
        self.write_short(len);
        self.data.extend_from_slice(s.as_bytes());
        Ok(())
    }

    pub fn write_bytes(&mut self, buf: &[u8]) {
        self.data.extend_from_slice(buf);
    }

    pub fn read_byte(&mut self) -> Result<i8, ByteBufferError> {
        self.ensure_readable(1)?;
        match self.data.get(self.read_pos) {
            Some(&b) => {
                self.read_pos += 1;
                Ok(b as i8)
            }
            None => Err(ByteBufferError::Underflow {
                needed: 1,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_ubyte(&mut self) -> Result<u8, ByteBufferError> {
        self.ensure_readable(1)?;
        match self.data.get(self.read_pos) {
            Some(&b) => {
                self.read_pos += 1;
                Ok(b)
            }
            None => Err(ByteBufferError::Underflow {
                needed: 1,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_short(&mut self) -> Result<i16, ByteBufferError> {
        self.ensure_readable(2)?;
        let end = self.read_pos + 2;
        match self.data.get(self.read_pos..end) {
            Some(slice) => match <[u8; 2]>::try_from(slice) {
                Ok(arr) => {
                    self.read_pos = end;
                    Ok(i16::from_be_bytes(arr))
                }
                Err(_) => Err(ByteBufferError::Underflow {
                    needed: 2,
                    remaining: self.remaining(),
                }),
            },
            None => Err(ByteBufferError::Underflow {
                needed: 2,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_int(&mut self) -> Result<i32, ByteBufferError> {
        self.ensure_readable(4)?;
        let end = self.read_pos + 4;
        match self.data.get(self.read_pos..end) {
            Some(slice) => match <[u8; 4]>::try_from(slice) {
                Ok(arr) => {
                    self.read_pos = end;
                    Ok(i32::from_be_bytes(arr))
                }
                Err(_) => Err(ByteBufferError::Underflow {
                    needed: 4,
                    remaining: self.remaining(),
                }),
            },
            None => Err(ByteBufferError::Underflow {
                needed: 4,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_long(&mut self) -> Result<i64, ByteBufferError> {
        self.ensure_readable(8)?;
        let end = self.read_pos + 8;
        match self.data.get(self.read_pos..end) {
            Some(slice) => match <[u8; 8]>::try_from(slice) {
                Ok(arr) => {
                    self.read_pos = end;
                    Ok(i64::from_be_bytes(arr))
                }
                Err(_) => Err(ByteBufferError::Underflow {
                    needed: 8,
                    remaining: self.remaining(),
                }),
            },
            None => Err(ByteBufferError::Underflow {
                needed: 8,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_float(&mut self) -> Result<f32, ByteBufferError> {
        let bits = self.read_int()?;
        Ok(f32::from_bits(bits as u32))
    }

    pub fn read_double(&mut self) -> Result<f64, ByteBufferError> {
        let bits = self.read_long()?;
        Ok(f64::from_bits(bits as u64))
    }

    pub fn read_bool(&mut self) -> Result<bool, ByteBufferError> {
        let b = self.read_byte()?;
        Ok(b != 0)
    }

    pub fn read_utf(&mut self) -> Result<String, ByteBufferError> {
        self.read_utf_with_max(32767)
    }

    pub fn read_utf_with_max(&mut self, max_length: i16) -> Result<String, ByteBufferError> {
        let len = self.read_short()?;
        if len < 0 {
            return Err(ByteBufferError::NegativeLength(len as i32));
        }
        if len > max_length {
            return Err(ByteBufferError::LengthExceedsMaximum {
                len: len as i32,
                max: max_length as i32,
            });
        }
        if len == 0 {
            return Ok(String::new());
        }
        let ulen = len as usize;
        self.ensure_readable(ulen)?;
        let end = self.read_pos + ulen;
        match self.data.get(self.read_pos..end) {
            Some(slice) => match std::str::from_utf8(slice) {
                Ok(s) => {
                    let out = s.to_owned();
                    self.read_pos = end;
                    Ok(out)
                }
                Err(_) => Err(ByteBufferError::InvalidUtf8),
            },
            None => Err(ByteBufferError::Underflow {
                needed: ulen,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>, ByteBufferError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        self.ensure_readable(len)?;
        let end = self.read_pos + len;
        match self.data.get(self.read_pos..end) {
            Some(slice) => {
                let out = slice.to_vec();
                self.read_pos = end;
                Ok(out)
            }
            None => Err(ByteBufferError::Underflow {
                needed: len,
                remaining: self.remaining(),
            }),
        }
    }

    pub fn read_bytes_into(&mut self, buf: &mut [u8]) -> Result<(), ByteBufferError> {
        if buf.is_empty() {
            return Ok(());
        }
        let len = buf.len();
        self.ensure_readable(len)?;
        let end = self.read_pos + len;
        match self.data.get(self.read_pos..end) {
            Some(slice) => {
                buf.copy_from_slice(slice);
                self.read_pos = end;
                Ok(())
            }
            None => Err(ByteBufferError::Underflow {
                needed: len,
                remaining: self.remaining(),
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;

    #[test]
    fn test_write_read_byte() {
        let mut buf = ByteBuffer::new();
        buf.write_byte(-128);
        buf.write_byte(127);
        assert_eq!(buf.read_pos, 0);
        assert_eq!(buf.read_byte().unwrap(), -128);
        assert_eq!(buf.read_byte().unwrap(), 127);
    }

    #[test]
    fn test_write_read_ubyte() {
        let mut buf = ByteBuffer::new();
        buf.write_ubyte(0);
        buf.write_ubyte(255);
        assert_eq!(buf.read_ubyte().unwrap(), 0);
        assert_eq!(buf.read_ubyte().unwrap(), 255);
    }

    #[test]
    fn test_write_read_short() {
        let mut buf = ByteBuffer::new();
        buf.write_short(-32768);
        buf.write_short(32767);
        buf.write_short(0);
        assert_eq!(buf.read_short().unwrap(), -32768);
        assert_eq!(buf.read_short().unwrap(), 32767);
        assert_eq!(buf.read_short().unwrap(), 0);
    }

    #[test]
    fn test_write_read_int() {
        let mut buf = ByteBuffer::new();
        buf.write_int(i32::MIN);
        buf.write_int(i32::MAX);
        buf.write_int(0);
        assert_eq!(buf.read_int().unwrap(), i32::MIN);
        assert_eq!(buf.read_int().unwrap(), i32::MAX);
        assert_eq!(buf.read_int().unwrap(), 0);
    }

    #[test]
    fn test_write_read_long() {
        let mut buf = ByteBuffer::new();
        buf.write_long(i64::MIN);
        buf.write_long(i64::MAX);
        buf.write_long(0);
        assert_eq!(buf.read_long().unwrap(), i64::MIN);
        assert_eq!(buf.read_long().unwrap(), i64::MAX);
        assert_eq!(buf.read_long().unwrap(), 0);
    }

    #[test]
    fn test_write_read_float() {
        let mut buf = ByteBuffer::new();
        buf.write_float(3.14159f32);
        buf.write_float(0.0);
        buf.write_float(-1.0);
        assert!((buf.read_float().unwrap() - 3.14159f32).abs() < 1e-6);
        assert!((buf.read_float().unwrap() - 0.0).abs() < 1e-6);
        assert!((buf.read_float().unwrap() - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_write_read_double() {
        let mut buf = ByteBuffer::new();
        buf.write_double(3.14159265358979);
        buf.write_double(0.0);
        assert!((buf.read_double().unwrap() - 3.14159265358979).abs() < 1e-12);
        assert!((buf.read_double().unwrap() - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_write_read_bool() {
        let mut buf = ByteBuffer::new();
        buf.write_bool(true);
        buf.write_bool(false);
        assert!(buf.read_bool().unwrap());
        assert!(!buf.read_bool().unwrap());
    }

    #[test]
    fn test_write_read_utf() {
        let mut buf = ByteBuffer::new();
        buf.write_utf("Hello, World!").unwrap();
        assert_eq!(buf.read_utf().unwrap(), "Hello, World!");
    }

    #[test]
    fn test_write_read_empty_utf() {
        let mut buf = ByteBuffer::new();
        buf.write_utf("").unwrap();
        assert_eq!(buf.read_utf().unwrap(), "");
    }

    #[test]
    fn test_write_read_utf_unicode() {
        let mut buf = ByteBuffer::new();
        buf.write_utf("Minecraft \u{00a7}aAlpha").unwrap();
        assert_eq!(buf.read_utf().unwrap(), "Minecraft \u{00a7}aAlpha");
    }

    #[test]
    fn test_write_read_bytes() {
        let mut buf = ByteBuffer::new();
        let d = vec![1u8, 2, 3, 4, 5];
        buf.write_bytes(&d);
        let r = buf.read_bytes(5).unwrap();
        assert_eq!(r.len(), 5);
        for i in 0..5 {
            assert_eq!(r[i], d[i]);
        }
    }

    #[test]
    fn test_big_endian_short() {
        let mut buf = ByteBuffer::new();
        buf.write_short(0x1234);
        assert!(buf.data.len() >= 2);
        assert_eq!(buf.data[0], 0x12);
        assert_eq!(buf.data[1], 0x34);
    }

    #[test]
    fn test_big_endian_int() {
        let mut buf = ByteBuffer::new();
        buf.write_int(0x12345678);
        assert_eq!(buf.data[0], 0x12);
        assert_eq!(buf.data[1], 0x34);
        assert_eq!(buf.data[2], 0x56);
        assert_eq!(buf.data[3], 0x78);
    }

    #[test]
    fn test_buffer_underflow() {
        let mut buf = ByteBuffer::new();
        buf.write_byte(42);
        assert_eq!(buf.read_byte().unwrap(), 42);
        assert!(buf.read_int().is_err());
    }

    #[test]
    fn test_remaining() {
        let mut buf = ByteBuffer::new();
        buf.write_short(100);
        buf.write_int(200);
        assert_eq!(buf.remaining(), 6);
        buf.read_short().unwrap();
        assert_eq!(buf.remaining(), 4);
    }

    #[test]
    fn test_read_write_span() {
        let mut buf = ByteBuffer::new();
        let arr = [10u8, 20, 30, 40];
        buf.write_bytes(&arr);
        let mut out = [0u8; 4];
        buf.read_bytes_into(&mut out).unwrap();
        for i in 0..4 {
            assert_eq!(out[i], arr[i]);
        }
    }

    #[test]
    fn test_empty_buffer_oob() {
        let mut buf = ByteBuffer::new();
        assert_eq!(buf.remaining(), 0);
        assert!(buf.is_empty());
        assert!(buf.read_byte().is_err());
        assert!(buf.read_ubyte().is_err());
        assert!(buf.read_short().is_err());
        assert!(buf.read_int().is_err());
        assert!(buf.read_long().is_err());
        assert!(buf.read_float().is_err());
        assert!(buf.read_double().is_err());
        assert!(buf.read_bool().is_err());
        assert!(buf.read_utf().is_err());
        assert!(buf.read_bytes(1).is_err());
    }

    #[test]
    fn test_oob_read_past_end() {
        let mut buf = ByteBuffer::new();
        buf.write_int(123);
        assert_eq!(buf.read_int().unwrap(), 123);
        assert_eq!(buf.remaining(), 0);
        assert!(buf.read_ubyte().is_err());
        assert!(buf.read_bytes(4).is_err());
        let mut out = [0u8; 2];
        assert!(buf.read_bytes_into(&mut out).is_err());
    }

    #[test]
    fn test_write_utf_too_long_rejected() {
        let mut buf = ByteBuffer::new();
        let big = "a".repeat(32768);
        assert!(buf.write_utf(&big).is_err());
        assert_eq!(buf.remaining(), 0);
    }

    #[test]
    fn test_read_utf_truncated_fails() {
        let mut buf = ByteBuffer::new();
        buf.write_short(5);
        buf.write_bytes(&[104, 105]);
        assert!(buf.read_utf().is_err());
    }

    #[test]
    fn test_read_utf_negative_len_fails() {
        let mut buf = ByteBuffer::from_vec(vec![0xFF, 0xFF]);
        assert!(buf.read_utf().is_err());
    }

    #[test]
    fn test_from_vec_and_cursor_reset() {
        let mut buf = ByteBuffer::from_vec(vec![0x00, 0x2A]);
        assert_eq!(buf.remaining(), 2);
        assert_eq!(buf.read_short().unwrap(), 42);
        assert_eq!(buf.remaining(), 0);
        buf.reset_cursor();
        assert_eq!(buf.remaining(), 2);
        assert_eq!(buf.read_short().unwrap(), 42);
    }
}
