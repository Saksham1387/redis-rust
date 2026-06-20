use crate::{
    cmd::Cmd,
    eval::eval::evaluate,
    resp::{decode::read_array_string, types::RespError},
    store::store::Store,
};
use mio::{
    net::{TcpListener, TcpStream},
    Events, Interest, Poll, Token,
};
use std::{
    collections::HashMap,
    env,
    io::{self, Cursor, ErrorKind, Read, Write},
    net::SocketAddr,

};

pub mod cmd;
pub mod eval;
pub mod resp;
pub mod store;

// Token(0) is reserved for the listener. Client connections get Token(1), Token(2), ...
const LISTENER: Token = Token(0);
const READ_BUF_SIZE: usize = 4096;

enum ParseResult {
    Complete(Cmd, usize),
    Incomplete,
}

fn parse_command(buf: &[u8]) -> Result<ParseResult, RespError> {
    if buf.is_empty() {
        return Ok(ParseResult::Incomplete);
    }
    let mut cursor = Cursor::new(buf);
    match read_array_string(&mut cursor) {
        Ok(mut parts) => {
            if parts.is_empty() {
                return Err(RespError::Protocol("Empty command".to_string()));
            }
            let cmd = parts.remove(0);
            let consumed = cursor.position() as usize;
            Ok(ParseResult::Complete(Cmd { cmd, args: parts }, consumed))
        }
        Err(RespError::Io(e)) if e.kind() == ErrorKind::UnexpectedEof => Ok(ParseResult::Incomplete),
        Err(e) => Err(e),
    }
}

// All state for a single client connection.
struct Conn {
    stream: TcpStream,
    read_buf: Vec<u8>,   // bytes received from the client, not yet parsed
    write_buf: Vec<u8>,  // response bytes waiting to be sent
}

fn main() -> io::Result<()> {
    let addr: SocketAddr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9090".to_string())
        .parse()
        .expect("Invalid address");

    // Poll wraps the OS mechanism: epoll on Linux, kqueue on macOS.
    // It keeps a table of (fd → token, interests) and lets us ask
    // "which of these fds changed state since I last checked?"
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(128);

    let mut listener = TcpListener::bind(addr)?;
    // Tell the OS: when this fd becomes readable (new connection waiting),
    // include it in the next poll result tagged as LISTENER.
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;

    let mut store = Store::new();
    let mut connections: HashMap<Token, Conn> = HashMap::new();
    let mut next_id: usize = 1;

    println!("Listening on {addr}");

    loop {
        // ── WAIT ──────────────────────────────────────────────────────────────
        // This is the heart of the event loop. We hand control to the OS here.
        // It returns only when at least one registered fd is ready.
        // `None` timeout means "block forever until something happens."
        poll.poll(&mut events, None)?;

        let mut to_close: Vec<Token> = Vec::new();

        // ── DISPATCH ──────────────────────────────────────────────────────────
        // Each event carries the Token we assigned at registration time,
        // plus flags for what happened (readable, writable, or both).
        for event in events.iter() {
            match event.token() {

                // ── NEW CONNECTION ─────────────────────────────────────────
                LISTENER => {
                    // The listener fd is readable, meaning ≥1 connection is in
                    // the accept queue. We must loop until WouldBlock because
                    // edge-triggered delivery won't fire again for queued
                    // connections we didn't drain this round.
                    loop {
                        match listener.accept() {
                            Ok((mut stream, peer)) => {
                                let token = Token(next_id);
                                next_id += 1;
                                println!("Accepted {peer} → {token:?}");
                                // Start by waiting for the client to send us something.
                                poll.registry()
                                    .register(&mut stream, token, Interest::READABLE)?;
                                connections.insert(token, Conn {
                                    stream,
                                    read_buf: Vec::new(),
                                    write_buf: Vec::new(),
                                });
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                            Err(e) => return Err(e),
                        }
                    }
                }

                // ── EXISTING CONNECTION ────────────────────────────────────
                token => {
                    let conn = match connections.get_mut(&token) {
                        Some(c) => c,
                        None => continue,
                    };

                    // ── READ ──────────────────────────────────────────────
                    // The socket has incoming bytes. Read until the OS says
                    // WouldBlock ("no more data right now, come back later").
                    if event.is_readable() {
                        let mut tmp = [0u8; READ_BUF_SIZE];
                        'read: loop {
                            match conn.stream.read(&mut tmp) {
                                Ok(0) => {
                                    // n=0 means the client closed the connection (EOF).
                                    to_close.push(token);
                                    break 'read;
                                }
                                Ok(n) => conn.read_buf.extend_from_slice(&tmp[..n]),
                                Err(e) if e.kind() == ErrorKind::WouldBlock => break 'read,
                                Err(_) => {
                                    to_close.push(token);
                                    break 'read;
                                }
                            }
                        }

                        if to_close.contains(&token) {
                            continue;
                        }

                        // Parse every complete RESP command sitting in read_buf.
                        // A single read() may have delivered multiple pipelined commands.
                        loop {
                            match parse_command(&conn.read_buf) {
                                Ok(ParseResult::Complete(cmd, consumed)) => {
                                    conn.read_buf.drain(..consumed);
                                    if evaluate(cmd, &mut conn.write_buf, &mut store)
                                        .is_err()
                                    {
                                        to_close.push(token);
                                        break;
                                    }
                                }
                                Ok(ParseResult::Incomplete) => break, // wait for more bytes
                                Err(err) => {
                                    conn.write_buf
                                        .extend_from_slice(format!("-ERR {err}\r\n").as_bytes());
                                    to_close.push(token);
                                    break;
                                }
                            }
                        }

                        // We have responses to send. Switch the interest to WRITABLE:
                        // the OS will tell us when the kernel send-buffer has room.
                        if !conn.write_buf.is_empty() && !to_close.contains(&token) {
                            poll.registry()
                                .reregister(&mut conn.stream, token, Interest::WRITABLE)?;
                        }
                    }

                    // ── WRITE ─────────────────────────────────────────────
                    // The kernel send-buffer has space. Push as many bytes as
                    // we can; stop when WouldBlock (buffer full again).
                    if event.is_writable() && !to_close.contains(&token) {
                        loop {
                            if conn.write_buf.is_empty() {
                                break;
                            }
                            match conn.stream.write(&conn.write_buf) {
                                Ok(n) => {
                                    // Partial writes are normal on non-blocking sockets.
                                    conn.write_buf.drain(..n);
                                }
                                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                                Err(_) => {
                                    to_close.push(token);
                                    break;
                                }
                            }
                        }

                        // All responses flushed — go back to listening for new commands.
                        if conn.write_buf.is_empty() && !to_close.contains(&token) {
                            poll.registry()
                                .reregister(&mut conn.stream, token, Interest::READABLE)?;
                        }
                        // If write_buf still has bytes we hit WouldBlock: keep WRITABLE
                        // registered, the OS will fire again when there's room.
                    }
                }
            }
        }

        // ── CLEANUP ───────────────────────────────────────────────────────────
        for token in to_close {
            if let Some(mut conn) = connections.remove(&token) {
                // Unregister from epoll/kqueue before dropping the fd.
                poll.registry().deregister(&mut conn.stream).ok();
                println!("Closed {token:?}");
            }
        }
    }
}
