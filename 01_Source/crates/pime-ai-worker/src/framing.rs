//! JSON Lines framing: one request per line, one reply per request, lines never interleave.
//!
//! The bounded reader is the same algorithm as `pime_core::protocol::read_bounded_line` (which is private there; the
//! worker keeps its own copy instead of editing the core): a line is never buffered beyond the limit, an oversized line
//! is skipped to its end and reported as too long, a trailing `\r` is dropped.
#![forbid(unsafe_code)]

use std::io::{self, BufRead};

/// What one read produced. The bytes are in the caller's buffer for `Line`.
#[derive(Debug, PartialEq, Eq)]
pub enum LineRead {
    Line,
    TooLong,
    Eof,
}

pub fn read_bounded_line<R: BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<LineRead> {
    buf.clear();
    let mut too_long = false;
    let mut seen_any = false;
    loop {
        let chunk = r.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if !seen_any {
                LineRead::Eof
            } else if too_long || buf.len() > max {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
        seen_any = true;
        let (take, found) = match chunk.iter().position(|&b| b == b'\n') {
            Some(p) => (p, true),
            None => (chunk.len(), false),
        };
        if !too_long {
            if buf.len() + take > max + 1 {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(take + usize::from(found));
        if found {
            if !too_long && buf.last() == Some(&b'\r') {
                buf.pop();
            }
            if buf.len() > max {
                too_long = true;
            }
            return Ok(if too_long {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn read_all(input: &[u8], max: usize) -> Vec<(LineRead, Vec<u8>)> {
        let mut r = Cursor::new(input.to_vec());
        let mut buf = Vec::new();
        let mut out = Vec::new();
        loop {
            let k = read_bounded_line(&mut r, &mut buf, max).unwrap();
            if k == LineRead::Eof {
                break;
            }
            out.push((k, buf.clone()));
        }
        out
    }

    #[test]
    fn lines_are_split_at_newlines_and_crlf_is_accepted() {
        let got = read_all(b"{\"a\":1}\r\n{\"b\":2}\n", 100);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (LineRead::Line, b"{\"a\":1}".to_vec()));
        assert_eq!(got[1], (LineRead::Line, b"{\"b\":2}".to_vec()));
    }

    #[test]
    fn a_last_line_without_newline_is_still_a_line() {
        let got = read_all(b"abc", 100);
        assert_eq!(got, vec![(LineRead::Line, b"abc".to_vec())]);
        assert!(read_all(b"", 100).is_empty());
    }

    #[test]
    fn an_oversized_line_is_skipped_to_its_end_and_the_next_line_survives() {
        let mut input = vec![b'x'; 50];
        input.push(b'\n');
        input.extend_from_slice(b"ok\n");
        let got = read_all(&input, 10);
        assert_eq!(got[0].0, LineRead::TooLong);
        assert_eq!(got[1], (LineRead::Line, b"ok".to_vec()));
        // exactly at the limit is fine, one byte over is not
        assert_eq!(read_all(&[b'y'; 10], 10)[0].0, LineRead::Line);
        assert_eq!(read_all(&[b'y'; 11], 10)[0].0, LineRead::TooLong);
    }

    #[test]
    fn an_oversized_line_never_stays_in_the_buffer() {
        let input = vec![b'z'; 1000];
        let mut r = Cursor::new(input);
        let mut buf = Vec::new();
        assert_eq!(
            read_bounded_line(&mut r, &mut buf, 16).unwrap(),
            LineRead::TooLong
        );
        assert!(buf.len() <= 17);
    }
}
