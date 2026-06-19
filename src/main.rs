use crate::{cmd::Cmd, eval::eval::evaluate, resp::types::RespError};
use std::{env, net::SocketAddr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub mod cmd;
pub mod eval;
pub mod resp;

const READ_BUFFER_SIZE: usize = 4096;

enum ParseResult {
    Complete(Cmd, usize),
    Incomplete,
}

fn find_crlf(buf: &[u8], start: usize) -> Option<usize> {
    buf.get(start..)?
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|pos| start + pos)
}

fn parse_line(buf: &[u8], pos: &mut usize) -> Result<Option<String>, RespError> {
    let Some(end) = find_crlf(buf, *pos) else {
        return Ok(None);
    };

    let line = std::str::from_utf8(&buf[*pos..end])
        .map_err(|_| RespError::Protocol("Invalid UTF-8".to_string()))?
        .to_string();
    *pos = end + 2;
    Ok(Some(line))
}

fn parse_command(buf: &[u8]) -> Result<ParseResult, RespError> {
    let mut pos = 0;

    if buf.is_empty() {
        return Ok(ParseResult::Incomplete);
    }

    if buf[pos] != b'*' {
        return Err(RespError::Protocol("Expected array".to_string()));
    }
    pos += 1;

    let Some(array_len) = parse_line(buf, &mut pos)? else {
        return Ok(ParseResult::Incomplete);
    };
    let array_len: i64 = array_len
        .parse()
        .map_err(|_| RespError::Protocol("Invalid array length".to_string()))?;

    if array_len <= 0 {
        return Err(RespError::Protocol("Empty command".to_string()));
    }

    let mut values = Vec::with_capacity(array_len as usize);
    for _ in 0..array_len {
        if pos >= buf.len() {
            return Ok(ParseResult::Incomplete);
        }

        match buf[pos] {
            b'+' => {
                pos += 1;
                let Some(value) = parse_line(buf, &mut pos)? else {
                    return Ok(ParseResult::Incomplete);
                };
                values.push(value);
            }
            b'$' => {
                pos += 1;
                let Some(len) = parse_line(buf, &mut pos)? else {
                    return Ok(ParseResult::Incomplete);
                };
                let len: i64 = len
                    .parse()
                    .map_err(|_| RespError::Protocol("Invalid bulk string length".to_string()))?;
                if len < 0 {
                    return Err(RespError::Protocol(
                        "Null bulk strings are not valid commands".to_string(),
                    ));
                }

                let len = len as usize;
                if buf.len() < pos + len + 2 {
                    return Ok(ParseResult::Incomplete);
                }
                if &buf[pos + len..pos + len + 2] != b"\r\n" {
                    return Err(RespError::Protocol("Bulk string missing CRLF".to_string()));
                }

                let value = std::str::from_utf8(&buf[pos..pos + len])
                    .map_err(|_| RespError::Protocol("Invalid UTF-8 in bulk string".to_string()))?
                    .to_string();
                values.push(value);
                pos += len + 2;
            }
            _ => {
                return Err(RespError::Protocol(
                    "Expected string command item".to_string(),
                ));
            }
        }
    }

    let cmd = values.remove(0);
    Ok(ParseResult::Complete(Cmd { cmd, args: values }, pos))
}

async fn handle_connection(mut socket: TcpStream) {
    let mut read_buf: Vec<u8> = Vec::new();
    let mut write_buf: Vec<u8> = Vec::new();
    let mut buf = [0u8; READ_BUFFER_SIZE];

    loop {
        // drain as many complete commands as are already buffered
        loop {
            match parse_command(&read_buf) {
                Ok(ParseResult::Complete(cmd, consumed)) => {
                    read_buf.drain(..consumed);
                    if let Err(err) = evaluate(cmd, &mut write_buf) {
                        eprintln!("Failed to build response: {err}");
                        return;
                    }
                }
                Ok(ParseResult::Incomplete) => break,
                Err(err) => {
                    write_buf.extend_from_slice(format!("-ERR {err}\r\n").as_bytes());
                    let _ = socket.write_all(&write_buf).await;
                    return;
                }
            }
        }

        if !write_buf.is_empty() {
            if let Err(err) = socket.write_all(&write_buf).await {
                eprintln!("write error: {err}");
                return;
            }
            write_buf.clear();
        }

        match socket.read(&mut buf).await {
            Ok(0) => return, // client closed connection
            Ok(n) => read_buf.extend_from_slice(&buf[..n]),
            Err(err) => {
                eprintln!("read error: {err}");
                return;
            }
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9090".to_string());
    let addr: SocketAddr = addr.parse().expect("Invalid address");

    let listener = TcpListener::bind(addr).await?;
    println!("Listening on {addr}");

    loop {
        let (socket, address) = listener.accept().await?;
        println!("Accepted connection from {address}");
        tokio::spawn(handle_connection(socket));
    }
}
