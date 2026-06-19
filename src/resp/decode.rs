use crate::resp::types::{RESPResponse, RespError};
use std::io::BufRead;

// It should return the string, delta and the error
fn read_simple_string<R: BufRead + ?Sized>(mut data: &mut R) -> Result<RESPResponse, RespError> {
    let result = read_line(&mut data)?;
    Ok(RESPResponse::SimpleString(result))
}

fn read_line<R: BufRead + ?Sized>(reader: &mut R) -> Result<String, RespError> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.ends_with("\r\n") {
        line.truncate(line.len() - 2);
    } else {
        return Err(RespError::Protocol("No end line detected".to_string()));
    }
    Ok(line)
}

fn read_int_64<R: BufRead + ?Sized>(mut data: &mut R) -> Result<RESPResponse, RespError> {
    let result = read_line(&mut data)?;
    let n = result
        .parse::<i64>()
        .map_err(|_| RespError::Protocol("Could not find a valid integer".to_string()))?;
    Ok(RESPResponse::Int64(n))
}

fn read_error<R: BufRead + ?Sized>(mut data: &mut R) -> Result<RESPResponse, RespError> {
    return read_simple_string(&mut data);
}

pub fn read_array_string<R: BufRead + ?Sized>(data: &mut R) -> Result<Vec<String>, RespError> {
    let result = decode(data)?;
    match result {
        RESPResponse::Array(Some(items)) => {
            let mut final_list = Vec::with_capacity(items.len());

            for item in items {
                match item {
                    RESPResponse::SimpleString(s) => final_list.push(s),
                    RESPResponse::BulkString(Some(bytes)) => {
                        let s = String::from_utf8(bytes).map_err(|_| {
                            RespError::Protocol("Invalid UTF-8 in bulk string".to_string())
                        })?;
                        final_list.push(s);
                    }

                    _ => {
                        return Err(RespError::Protocol(
                            "Expected a string or bulk string".to_string(),
                        ));
                    }
                }
            }
            Ok(final_list)
        }
        RESPResponse::Array(None) => Ok(Vec::new()),
        _ => Err(RespError::Protocol("Expected array".to_string())),
    }
}

fn read_bulk_string<R: BufRead + ?Sized>(mut data: &mut R) -> Result<RESPResponse, RespError> {
    let len: i64 = read_line(&mut data)?
        .parse()
        .map_err(|_| RespError::Protocol("invalid bulk string length".into()))?;

    if len == -1 {
        return Ok(RESPResponse::BulkString(None)); // null bulk string
    }

    let len = len as usize;
    let mut buf = vec![0u8; len + 2]; // +2 for \r\n
    data.read_exact(&mut buf)?;
    buf.truncate(len);
    Ok(RESPResponse::BulkString(Some(buf)))
}

fn read_array<R: BufRead + ?Sized>(mut data: &mut R) -> Result<RESPResponse, RespError> {
    let len: i64 = read_line(&mut data)?
        .parse()
        .map_err(|_| RespError::Protocol("invalid bulk string length".into()))?;

    if len == -1 {
        return Ok(RESPResponse::Array(None)); // null array
    }

    let mut items = Vec::with_capacity(len as usize);
    for _ in 0..len {
        items.push(decode(data)?);
    }

    Ok(RESPResponse::Array(Some(items)))
}

pub fn decode<R: BufRead + ?Sized>(data: &mut R) -> Result<RESPResponse, RespError> {
    let mut first_byte = [0u8; 1];
    data.read_exact(&mut first_byte)?;

    match first_byte[0] {
        b'+' => return read_simple_string(data),
        b'-' => return read_error(data),
        b':' => return read_int_64(data),
        b'$' => read_bulk_string(data),
        b'*' => read_array(data),
        other => Err(RespError::Protocol(format!(
            "unknown type byte: {:?}",
            other as char
        ))),
    }
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn test_simple_string() {
        let input = b"+DELETE\r\n";
        let mut reader = Cursor::new(input);
        let result = decode(&mut reader).unwrap();

        assert_eq!(result, RESPResponse::SimpleString("DELETE".to_string()));
    }

    #[test]
    fn test_int64() {
        let input = b":456\r\n";
        let mut reader = Cursor::new(input);
        let result = decode(&mut reader).unwrap();

        assert_eq!(result, RESPResponse::Int64(456));
    }

    #[test]
    fn test_bulk_string() {
        let input = b"$4\r\nPONG\r\n";
        let mut reader = Cursor::new(input);
        let result = decode(&mut reader).unwrap();

        assert_eq!(result, RESPResponse::BulkString(Some(b"PONG".to_vec())));
    }

    #[test]
    fn test_null_bulk_string() {
        let input = b"$-1\r\n";
        let mut reader = Cursor::new(input);
        let result = decode(&mut reader).unwrap();

        assert_eq!(result, RESPResponse::BulkString(None));
    }
}
