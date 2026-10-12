pub const SIGN_LINES: usize = 4;

#[derive(Clone, Copy, Debug)]
pub struct SignState {
    pub lines: [[u8; 16]; SIGN_LINES],
}

pub fn sign_create() -> SignState {
    SignState {
        lines: [[0u8; 16]; SIGN_LINES],
    }
}

/// Store one sign line (from client packets or chunk blobs), always
/// through [`sign_sanitize_line`].
pub fn sign_set_line(state: &mut SignState, line: i32, text: &str) {
    if line < 0 || line as usize >= SIGN_LINES {
        return;
    }
    state.lines[line as usize] = sign_sanitize_line(text);
}

/// Sanitized 15-byte storage form of one sign line.
///
/// The vanilla client parses sign NBT through Java `DataInput.readUTF`,
/// which is *modified* UTF-8: a raw NUL byte or any astral-plane
/// character (4-byte standard UTF-8, e.g. emoji) throws
/// `UTFDataFormatException` and disconnects the client. Without this
/// filter a single crafted sign-update packet poisons the tile
/// permanently — every player that receives the chunk gets kicked, on
/// every future load, until the bytes are wiped from the save.
/// Offending characters become `?`; a line holds at most 15 bytes and
/// never splits a multi-byte character.
pub fn sign_sanitize_line(text: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut len = 0usize;
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let cp = ch as u32;
        let encoded: &str = if cp == 0 || cp > 0xFFFF {
            "?"
        } else {
            ch.encode_utf8(&mut buf)
        };
        let b = encoded.as_bytes();
        if len + b.len() > 15 {
            break;
        }
        out[len..len + b.len()].copy_from_slice(b);
        len += b.len();
    }
    out
}
