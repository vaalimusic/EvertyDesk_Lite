use std::io::{self, BufRead};

pub const MAX_IPC_LINE_BYTES: usize = 64 * 1024;

/// Reads one UTF-8 line without allowing an untrusted peer to grow the buffer
/// beyond `max_bytes`. The returned string excludes CR/LF terminators.
pub fn read_bounded_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> io::Result<Option<String>> {
    let mut encoded = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if encoded.is_empty() {
                return Ok(None);
            }
            break;
        }

        let newline = available.iter().position(|byte| *byte == b'\n');
        let chunk_len = newline.unwrap_or(available.len());
        if encoded.len().saturating_add(chunk_len) > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("IPC message exceeds {max_bytes} bytes"),
            ));
        }
        encoded.extend_from_slice(&available[..chunk_len]);
        reader.consume(chunk_len + usize::from(newline.is_some()));
        if newline.is_some() {
            break;
        }
    }

    if encoded.last() == Some(&b'\r') {
        encoded.pop();
    }
    String::from_utf8(encoded)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_crlf_and_preserves_the_next_message() {
        let mut reader = Cursor::new(b"first\r\nsecond\n");
        assert_eq!(
            read_bounded_line(&mut reader, 16).unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(
            read_bounded_line(&mut reader, 16).unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(read_bounded_line(&mut reader, 16).unwrap(), None);
    }

    #[test]
    fn accepts_an_unterminated_final_line() {
        let mut reader = Cursor::new(b"final");
        assert_eq!(
            read_bounded_line(&mut reader, 5).unwrap().as_deref(),
            Some("final")
        );
    }

    #[test]
    fn rejects_oversized_and_invalid_utf8_messages() {
        let mut oversized = Cursor::new(b"123456\n");
        assert_eq!(
            read_bounded_line(&mut oversized, 5).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut invalid_utf8 = Cursor::new([0xff, b'\n']);
        assert_eq!(
            read_bounded_line(&mut invalid_utf8, 5).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
