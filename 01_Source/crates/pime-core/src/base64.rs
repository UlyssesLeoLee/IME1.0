//! Base64: the encoding that carries binary data through text (RFC 4648).
//!
//! The voice input needs it twice: the audio is base64 on its way into the request, and the request body itself is
//! base64 when it travels through a channel that only speaks text.
//!
//! It is here, in the core, because the worker and the text service must agree on it byte for byte.

/// The alphabet of the encoding.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The value of a character, or None when it is not part of the encoding.
fn value_of(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The text of the bytes, without the line breaks.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let three = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(three >> 18) as usize & 63] as char);
        out.push(ALPHABET[(three >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(three >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[three as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The bytes of the text.
///
/// None when the text is not base64: a wrong length, a character that is not part of the alphabet, or padding in the
/// wrong place. A body that cannot be decoded must never be sent half decoded.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let padding = chunk.iter().filter(|b| **b == b'=').count();
        if padding > 2 {
            return None;
        }
        // the padding is only allowed at the end of the last chunk
        if padding > 0 && chunk.as_ptr() != bytes[bytes.len() - 4..].as_ptr() {
            return None;
        }
        let mut value = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            let part = if *byte == b'=' {
                if index < 4 - padding {
                    return None;
                }
                0
            } else {
                value_of(*byte)?
            };
            value = (value << 6) | part as u32;
        }
        out.push((value >> 16) as u8);
        if padding < 2 {
            out.push((value >> 8) as u8);
        }
        if padding < 1 {
            out.push(value as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_examples_of_the_standard_survive() {
        for (text, bytes) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9vYmE=", b"fooba"),
            ("Zm9vYmFy", b"foobar"),
        ] {
            assert_eq!(encode(bytes), text, "{bytes:?}");
            assert_eq!(decode(text).as_deref(), Some(bytes), "{text}");
        }
    }

    #[test]
    fn every_byte_survives_the_round_trip() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(decode(&encode(&all)).as_deref(), Some(all.as_slice()));
    }

    #[test]
    fn text_that_is_not_base64_is_refused() {
        for text in [
            "Zg=",     // the length is wrong
            "Zg==Zg==", // a second group is not there
            "Zm9v!",  // a character that is not part of the alphabet
            "=Zm9",   // the padding is at the front
            "Zg==Zg==",
        ] {
            assert_eq!(decode(text), None, "{text}");
        }
    }

    #[test]
    fn the_line_breaks_of_a_long_text_do_not_matter() {
        let bytes: Vec<u8> = (0..200u8).collect();
        let text = encode(&bytes);
        let wrapped: String = text
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8_lossy(c).to_string())
            .collect::<Vec<_>>()
            .join("\r\n");
        assert_eq!(decode(&wrapped).as_deref(), Some(bytes.as_slice()));
    }
}
