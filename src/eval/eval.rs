use crate::{cmd::Cmd, store::store::{RedisValue, Store}};
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

fn eval_set<W: Write>(args: Vec<String>, writer: &mut W, store: &mut Store) -> io::Result<()> {
    if args.len() <= 1 {
        writer.write_all(b"-ERR wrong number of arguments for 'set' command\r\n")?;
    }

    let (key, value) = (args[0].clone(), args[1].clone());
    let mut ex_duration_ms: i64 = -1;
    let mut i = 2;
    
    while i < args.len() {
        match args[i].as_str() {
            "EX" => {
                i += 1;
                if i == args.len() {
                    writer.write_all(b"-ERR syntax error\r\n")?;
                    return Ok(());
                }
                let secs: i64 = match args[i].parse() {
                    Ok(n) => n,
                    Err(_) => {
                        writer.write_all(b"-ERR value is not an integer or out of range\r\n")?;
                        return Ok(());
                    }
                };
                ex_duration_ms = secs * 1000;
            }
            _ => {
                writer.write_all(b"-ERR syntax error\r\n")?;
                return Ok(());
            }
        }
        i += 1;
    }

    let duration = if ex_duration_ms == -1 { None } else { Some(ex_duration_ms) };
    store.set(key, RedisValue::String(value), duration);

    writer.write_all(b"+OK\r\n")?;
    Ok(())
}

fn eval_get<W: Write>(args: Vec<String>, writer: &mut W, store: &mut Store) -> io::Result<()> {
    if args.len() != 1 {
        writer.write_all(b"-ERR wrong number of arguments for 'get' command\r\n")?;
        return Ok(());
    }

    let result = match store.get(&args[0]) {
        Some(obj) => match &obj.value {
            crate::store::store::RedisValue::String(s) => encode(s, false),
            _ => b"-ERR wrong type\r\n".to_vec(),
        },
        None => b"$-1\r\n".to_vec(),
    };

    writer.write_all(&result)?;
    Ok(())
}

fn eval_ttl<W: Write>(args: Vec<String>, writer: &mut W, store: &mut Store) -> io::Result<()> {
    if args.len() != 1 {
        writer.write_all(b"-ERR wrong number of arguments for 'ttl' command\r\n")?;
        return Ok(());
    }

    let reply = match store.get(&args[0]) {
        None => b":-2\r\n".to_vec(),
        Some(obj) if obj.expires_at == -1 => b":-1\r\n".to_vec(),
        Some(obj) => {
            use std::time::{SystemTime, UNIX_EPOCH};
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
            let ttl_secs = (obj.expires_at - now) / 1000;
            format!(":{}\r\n", ttl_secs).into_bytes()
        }
    };

    writer.write_all(&reply)?;
    Ok(())
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

pub fn evaluate<W: Write>(cmd: Cmd, writer: &mut W, store: &mut Store) -> std::io::Result<()> {
    match cmd.cmd.as_str() {
        "PING" => {
            eval_ping(cmd.args, writer)?;
        }

        "GET" => {
            eval_get(cmd.args, writer, store)?;
        }

        "SET" => {
            eval_set(cmd.args, writer, store)?;
        }

        "TTL" => {
            eval_ttl(cmd.args, writer, store)?;
        }

        _ => {
            let err_msg = format!("-ERR unknown command '{}'\r\n", cmd.cmd);
            writer.write_all(err_msg.as_bytes())?;
        }
    }

    Ok(())
}
