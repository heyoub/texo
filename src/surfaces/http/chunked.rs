//! HTTP chunked transfer decoding.

use std::io::{BufRead, ErrorKind, Read};

use crate::surfaces::http::client::HttpClientError;

/// Decode an HTTP chunked response body.
///
/// # Errors
///
/// Returns [`HttpClientError::MalformedResponse`] for malformed framing,
/// [`HttpClientError::BodyTooLarge`] when the decoded body exceeds `body_cap`,
/// or [`HttpClientError::Io`] when the reader fails.
pub fn decode_chunked<R: BufRead>(
    reader: &mut R,
    body_cap: usize,
) -> Result<Vec<u8>, HttpClientError> {
    let mut body = Vec::new();
    loop {
        let line = read_line(reader)?;
        let size_text = line
            .split_once(';')
            .map_or(line.as_str(), |(head, _)| head)
            .trim();
        if size_text.is_empty() {
            return Err(HttpClientError::MalformedResponse {
                detail: "empty chunk size".to_string(),
            });
        }
        let size = usize::from_str_radix(size_text, 16).map_err(|_| {
            HttpClientError::MalformedResponse {
                detail: "invalid chunk size".to_string(),
            }
        })?;
        if size == 0 {
            consume_trailers(reader)?;
            return Ok(body);
        }
        if body.len().saturating_add(size) > body_cap {
            return Err(HttpClientError::BodyTooLarge);
        }
        let start = body.len();
        body.resize(start + size, 0);
        read_exact_chunk(reader, &mut body[start..], "chunk data")?;
        let mut crlf = [0_u8; 2];
        read_exact_chunk(reader, &mut crlf, "chunk delimiter")?;
        if crlf != *b"\r\n" {
            return Err(HttpClientError::MalformedResponse {
                detail: "chunk data was not followed by CRLF".to_string(),
            });
        }
    }
}

fn read_exact_chunk<R: Read>(
    reader: &mut R,
    buffer: &mut [u8],
    during: &'static str,
) -> Result<(), HttpClientError> {
    reader.read_exact(buffer).map_err(|source| {
        if source.kind() == ErrorKind::UnexpectedEof {
            HttpClientError::MalformedResponse {
                detail: format!("truncated {during}"),
            }
        } else {
            HttpClientError::Io { during, source }
        }
    })
}

fn consume_trailers<R: BufRead>(reader: &mut R) -> Result<(), HttpClientError> {
    loop {
        let line = read_line(reader)?;
        if line.is_empty() {
            return Ok(());
        }
    }
}

fn read_line<R: BufRead>(reader: &mut R) -> Result<String, HttpClientError> {
    let mut bytes = Vec::new();
    let read = reader
        .read_until(b'\n', &mut bytes)
        .map_err(|source| HttpClientError::Io {
            during: "chunk line",
            source,
        })?;
    if read == 0 {
        return Err(HttpClientError::MalformedResponse {
            detail: "truncated chunked body".to_string(),
        });
    }
    if !bytes.ends_with(b"\r\n") {
        return Err(HttpClientError::MalformedResponse {
            detail: "chunk line missing CRLF".to_string(),
        });
    }
    bytes.truncate(bytes.len() - 2);
    String::from_utf8(bytes).map_err(|_| HttpClientError::MalformedResponse {
        detail: "chunk line was not UTF-8".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn decodes_simple_chunks() {
        let mut input = Cursor::new(b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n".as_slice());
        assert_eq!(
            decode_chunked(&mut input, 1024).expect("chunked body decodes"),
            b"Wikipedia"
        );
    }

    #[test]
    fn decodes_extensions_and_trailers() {
        let mut input =
            Cursor::new(b"3;foo=bar\r\none\r\n3\r\ntwo\r\n0\r\nX-Debug: yes\r\n\r\n".as_slice());
        assert_eq!(
            decode_chunked(&mut input, 1024).expect("chunked body decodes"),
            b"onetwo"
        );
    }

    #[test]
    fn truncated_stream_is_malformed() {
        let mut input = Cursor::new(b"4\r\nWi".as_slice());
        let error = decode_chunked(&mut input, 1024).expect_err("truncated body rejected");
        assert!(matches!(error, HttpClientError::MalformedResponse { .. }));
    }

    #[test]
    fn size_line_overflow_is_malformed() {
        let mut input =
            Cursor::new(b"ffffffffffffffffffffffffffffffff\r\nnope\r\n0\r\n\r\n".as_slice());
        let error = decode_chunked(&mut input, 1024).expect_err("oversized size rejected");
        assert!(matches!(error, HttpClientError::MalformedResponse { .. }));
    }

    #[test]
    fn cap_is_enforced() {
        let mut input = Cursor::new(b"5\r\nhello\r\n0\r\n\r\n".as_slice());
        let error = decode_chunked(&mut input, 4).expect_err("body cap enforced");
        assert!(matches!(error, HttpClientError::BodyTooLarge));
    }
}
