use crate::{cmd::Cmd, eval::eval::evaluate, resp::{decode::read_array_string, types::RespError}};
use mio::{
    Events, Interest, Poll, Registry, Token,
    event::Event,
    net::{TcpListener, TcpStream},
};
use std::{
    collections::HashMap,
    env,
    io::{self, BufRead, Read, Write},
    net::SocketAddr,
};

pub mod cmd;
pub mod eval;
pub mod resp;

const SERVER: Token = Token(0);
const READ_BUFFER_SIZE: usize = 4096;

struct Connection {
    socket: TcpStream,
    read_buf: Vec<u8>,
    write_buf: Vec<u8>,
    closing: bool,
}

impl Connection {
    fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            read_buf: Vec::new(),
            write_buf: Vec::new(),
            closing: false,
        }
    }

    fn interest(&self) -> Interest {
        if self.write_buf.is_empty() {
            Interest::READABLE
        } else {
            Interest::READABLE.add(Interest::WRITABLE)
        }
    }
}

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

fn queue_response(connection: &mut Connection, cmd: Cmd) {
    if let Err(err) = evaluate(cmd, &mut connection.write_buf) {
        eprintln!("Failed to build response: {err}");
        connection.closing = true;
    }
}

fn handle_connection_event(
    registry: &Registry,
    token: Token,
    connection: &mut Connection,
    event: &Event,
) -> io::Result<bool> {
    if event.is_readable() {
        let mut buf = [0; READ_BUFFER_SIZE];

        loop {
            match connection.socket.read(&mut buf) {
                Ok(0) => {
                    connection.closing = true;
                    break;
                }
                Ok(n) => {
                    connection.read_buf.extend_from_slice(&buf[..n]);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }

        loop {
            match parse_command(&connection.read_buf) {
                Ok(ParseResult::Complete(cmd, consumed)) => {
                    connection.read_buf.drain(..consumed);
                    queue_response(connection, cmd);
                }
                Ok(ParseResult::Incomplete) => break,
                Err(err) => {
                    connection
                        .write_buf
                        .extend_from_slice(format!("-ERR {err}\r\n").as_bytes());
                    connection.read_buf.clear();
                    connection.closing = true;
                    break;
                }
            }
        }
    }

    if event.is_writable() {
        while !connection.write_buf.is_empty() {
            match connection.socket.write(&connection.write_buf) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    connection.write_buf.drain(..n);
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
    }

    if connection.closing && connection.write_buf.is_empty() {
        return Ok(true);
    }

    let interest = connection.interest();
    registry.reregister(&mut connection.socket, token, interest)?;
    Ok(false)
}

fn next_token(next: &mut Token) -> Token {
    let token = *next;
    next.0 += 1;
    token
}



// Register the client with a token ID, so that when the mio returns that this client is available to read it return with that id to identify the client
fn main() -> io::Result<()> {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9090".to_string());
    let addr = addr.parse().expect("Invalid address");

    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(128);
    let mut listener = TcpListener::bind(addr)?;
    poll.registry()
        .register(&mut listener, SERVER, Interest::READABLE)?;

    let mut connections = HashMap::new();
    let mut unique_token = Token(SERVER.0 + 1);

    println!("Listening on {addr}");

    loop {
        poll.poll(&mut events, None)?;

        for event in events.iter() {
            match event.token() {
                SERVER => loop {
                    let (mut socket, address) = match listener.accept() {
                        Ok((socket, address)) => (socket, address),
                        Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                        Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                        Err(err) => return Err(err),
                    };

                    let token = next_token(&mut unique_token);
                    poll.registry()
                        .register(&mut socket, token, Interest::READABLE)?;
                    connections.insert(token, Connection::new(socket));
                    println!("Accepted connection from {address}");
                },
                token => {
                    let done = if let Some(connection) = connections.get_mut(&token) {
                        handle_connection_event(poll.registry(), token, connection, event)?
                    } else {
                        false
                    };

                    if done {
                        if let Some(mut connection) = connections.remove(&token) {
                            poll.registry().deregister(&mut connection.socket)?;
                        }
                    }
                }
            }
        }
    }
}
