//! Deletable client half for the `netbat` request/response protocol.
//!
//! `netbat` 0.10 publishes request and response encoders plus a bounded server,
//! but no public response decoder or blocking client call. This module is the
//! narrow workaround tracked by freebatteryfactory/batpak#228.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// One successfully decoded remote operation output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    bytes: Vec<u8>,
}

impl Response {
    /// Consume the response and return its operation bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Stable failure classes for the temporary client boundary.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// Endpoint is not one concrete IP socket address.
    #[error("endpoint must be an IP socket address: {0}")]
    InvalidEndpoint(String),
    /// Connection, read, or write failed.
    #[error("transport {class}: {detail}")]
    Transport {
        /// Stable transport phase.
        class: &'static str,
        /// Sanitized I/O detail.
        detail: String,
    },
    /// The absolute request deadline elapsed.
    #[error("remote request deadline exceeded")]
    Deadline,
    /// The peer returned a malformed response frame.
    #[error("malformed netbat response: {0}")]
    Malformed(&'static str),
    /// The peer returned a bounded stable error frame.
    #[error("remote operation failed ({code}): {message}")]
    Remote {
        /// Stable `netbat` error token.
        code: String,
        /// Bounded UTF-8-lossy message.
        message: String,
    },
    /// The response exceeded the configured boundary.
    #[error("remote response exceeds {max} bytes")]
    TooLarge {
        /// Configured decoded or wire cap.
        max: usize,
    },
}

/// Perform one bounded `NETBAT/1 CALL` against an IP socket endpoint.
///
/// # Errors
/// Returns typed endpoint, transport, deadline, framing, remote-operation, or
/// size failures. The absolute deadline covers connect, all writes, and all
/// reads; a trickling peer cannot refresh it.
pub fn call(
    endpoint: &str,
    operation: &str,
    input: &[u8],
    limits: &netbat::Limits,
    timeout: Duration,
) -> Result<Response, ClientError> {
    if input.len() > limits.max_input_bytes {
        return Err(ClientError::TooLarge {
            max: limits.max_input_bytes,
        });
    }
    let address = endpoint
        .parse::<SocketAddr>()
        .map_err(|_| ClientError::InvalidEndpoint(endpoint.to_string()))?;
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| transport("connect", &error))?;
    let request = netbat::encode_request(operation, input);
    write_before_deadline(&mut stream, &request, started, timeout)?;
    let line = read_line_before_deadline(&mut stream, started, timeout, limits.max_line_bytes)?;
    decode_response(&line, limits)
}

/// Decode one complete `netbat` response line.
///
/// # Errors
/// Returns a malformed, remote-operation, or size failure for invalid input.
pub fn decode_response(line: &[u8], limits: &netbat::Limits) -> Result<Response, ClientError> {
    if line.len() > limits.max_line_bytes {
        return Err(ClientError::TooLarge {
            max: limits.max_line_bytes,
        });
    }
    let body = line
        .strip_suffix(b"\n")
        .ok_or(ClientError::Malformed("missing newline"))?;
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    let mut fields = body.split(|byte| *byte == b' ');
    match fields.next() {
        Some(b"OK") => {
            let payload = fields
                .next()
                .ok_or(ClientError::Malformed("missing OK payload"))?;
            if fields.next().is_some() {
                return Err(ClientError::Malformed("extra OK fields"));
            }
            if payload.len() / 2 > limits.max_output_bytes {
                return Err(ClientError::TooLarge {
                    max: limits.max_output_bytes,
                });
            }
            let bytes = netbat::decode_hex(payload, limits.max_output_bytes)
                .map_err(|_| ClientError::Malformed("invalid OK payload"))?;
            Ok(Response { bytes })
        }
        Some(b"ERR") => {
            let code = fields
                .next()
                .ok_or(ClientError::Malformed("missing ERR code"))?;
            let message = fields
                .next()
                .ok_or(ClientError::Malformed("missing ERR message"))?;
            if fields.next().is_some() {
                return Err(ClientError::Malformed("extra ERR fields"));
            }
            let code = std::str::from_utf8(code)
                .map_err(|_| ClientError::Malformed("ERR code is not UTF-8"))?;
            if code.is_empty()
                || !code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            {
                return Err(ClientError::Malformed("invalid ERR code"));
            }
            let message = netbat::decode_hex(message, limits.max_stream_error_message_bytes)
                .map_err(|_| ClientError::Malformed("invalid ERR message"))?;
            Err(ClientError::Remote {
                code: code.to_string(),
                message: String::from_utf8_lossy(&message).into_owned(),
            })
        }
        _ => Err(ClientError::Malformed("unknown response status")),
    }
}

fn write_before_deadline(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    started: Instant,
    timeout: Duration,
) -> Result<(), ClientError> {
    while !bytes.is_empty() {
        let remaining = remaining(started, timeout)?;
        stream
            .set_write_timeout(Some(remaining))
            .map_err(|error| transport("write_timeout", &error))?;
        let written = stream
            .write(bytes)
            .map_err(|error| classify_io("write", &error))?;
        if written == 0 {
            return Err(ClientError::Transport {
                class: "write",
                detail: "peer accepted zero bytes".to_string(),
            });
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

fn read_line_before_deadline(
    stream: &mut TcpStream,
    started: Instant,
    timeout: Duration,
    max: usize,
) -> Result<Vec<u8>, ClientError> {
    let mut line = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let remaining = remaining(started, timeout)?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|error| transport("read_timeout", &error))?;
        let count = stream
            .read(&mut buffer)
            .map_err(|error| classify_io("read", &error))?;
        if count == 0 {
            return Err(ClientError::Malformed("EOF before newline"));
        }
        let end = buffer[..count]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(count, |offset| offset.saturating_add(1));
        if line.len().saturating_add(end) > max {
            return Err(ClientError::TooLarge { max });
        }
        line.extend_from_slice(&buffer[..end]);
        if line.last() == Some(&b'\n') {
            return Ok(line);
        }
    }
}

fn remaining(started: Instant, timeout: Duration) -> Result<Duration, ClientError> {
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(ClientError::Deadline)
}

fn classify_io(class: &'static str, error: &std::io::Error) -> ClientError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        ClientError::Deadline
    } else {
        transport(class, error)
    }
}

fn transport(class: &'static str, error: &std::io::Error) -> ClientError {
    ClientError::Transport {
        class,
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_upstream_success_and_error_frames() {
        let limits = netbat::Limits::default();
        let ok = netbat::encode_response(Ok(b"hello"));
        assert_eq!(
            decode_response(&ok, &limits)
                .expect("upstream OK frame")
                .into_bytes(),
            b"hello"
        );
        let upstream = netbat::NetbatError::MalformedRequest { reason: "bad" };
        let error = decode_response(&netbat::encode_response(Err(&upstream)), &limits)
            .expect_err("upstream ERR frame");
        assert!(matches!(error, ClientError::Remote { .. }));
    }

    #[test]
    fn response_decoder_fails_closed_on_malformed_and_oversized_frames() {
        let limits = netbat::Limits::default().with_max_output_bytes(2);
        for malformed in [
            b"OK 00".as_slice(),
            b"OK zz\n".as_slice(),
            b"OK 00 extra\n".as_slice(),
            b"ERR BAD 00\n".as_slice(),
            b"NO 00\n".as_slice(),
        ] {
            assert!(decode_response(malformed, &limits).is_err());
        }
        assert!(matches!(
            decode_response(b"OK 000102\n", &limits),
            Err(ClientError::Malformed(_) | ClientError::TooLarge { .. })
        ));
    }
}
