use std::{
    env,
    io::{self, BufRead, Write},
    net::{TcpListener, TcpStream},
};
use crate::{
    cmd::Cmd, eval::eval::evaluate, resp::{decode::read_array_string, types::RespError}
};

pub mod cmd;
pub mod eval;
pub mod resp;

fn read_command<R: BufRead>(buffer: &mut R) -> Result<Cmd, RespError> {
    let mut values = read_array_string(buffer)?;
    if values.is_empty() {
        return Err(RespError::Protocol("Empty command".to_string()));
    }
    let cmd = values.remove(0);
    let args = values;

    Ok(Cmd {
        cmd: cmd,
        args: args,
    })
}

fn respond<W: Write>(cmd: Cmd, writer: &mut W) {
    if let Err(e) = evaluate(cmd, writer) {
        eprintln!("Failed to send response: {}", e);
    }
}

fn handle_client(stream: TcpStream) {
    let peer_addr = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_string(), |addr| addr.to_string());
    println!("Handling connection from {}", peer_addr);

    let mut reader = io::BufReader::new(stream);

    loop {
        match read_command(&mut reader) {
            Ok(cmd) => {
                println!("Received command");

                respond(cmd, reader.get_mut())
            }
            Err(_) => {
                break;
            }
        }
    }

    println!("Connection finished for: {}", peer_addr);
}

fn main() {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9090".to_string());

    let listener = TcpListener::bind(&addr).expect("Failed to bind the port");

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                handle_client(stream);
            }
            Err(e) => {
                eprint!("Failed to handle the incoming stream: {}", e)
            }
        }
    }
}
