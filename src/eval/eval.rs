use crate::cmd::Cmd;
use std::io::{self, Write};

fn encode(value: &str, is_simple: bool) -> Vec<u8> {
    if is_simple {
        // Simple String: +VALUE\r\n
        format!("+{}\r\n", value).into_bytes()
    } else {
        // Bulk String: $LEN\r\nVALUE\r\n
        format!("${}\r\n{}\r\n", value.len(), value).into_bytes()
    }
}

fn eval_ping<W: Write>(args: Vec<String>, writer: &mut W) -> io::Result<()> {
    if args.len() >= 2 {
        writer.write_all(b"-ERR wrong number of arguments for 'ping' command\r\n")?;
    }

    let result = if args.is_empty() {
        encode("PONG", true)
    } else {
        encode(&args[0], false)
    };

    writer.write_all(&result)?;
    writer.flush()?;

    Ok(())
}

pub fn evaluate<W: Write>(cmd: Cmd, writer: &mut W) -> std::io::Result<()> {
    match cmd.cmd.as_str() {
        "PING" => {
            eval_ping(cmd.args, writer)?;
        }

        _ => {
            let err_msg = format!("-ERR unknown command '{}'\r\n", cmd.cmd);
            writer.write_all(err_msg.as_bytes())?;
        }
    }

    Ok(())
}
