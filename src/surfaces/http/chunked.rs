//! HTTP chunked transfer decoding.

pub use super::codec::decode_chunked;

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::surfaces::http::client::HttpClientError;

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
