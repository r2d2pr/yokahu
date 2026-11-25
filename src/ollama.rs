use crate::json::{self, Json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub fn ollama_host_port() -> (String, u16) {
    let raw = std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "127.0.0.1:11434".to_string());
    let trimmed = raw
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    if let Some((h, p)) = trimmed.rsplit_once(':') {
        if let Ok(port) = p.parse::<u16>() {
            let host = if h.is_empty() || h == "0.0.0.0" { "127.0.0.1" } else { h };
            return (host.to_string(), port);
        }
    }
    ("127.0.0.1".to_string(), 11434)
}

pub fn connect_ollama(timeout_secs: u64) -> Result<TcpStream, String> {
    let (host, port) = ollama_host_port();
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .map_err(|e| format!("Invalid Ollama address: {}", e))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("Connect failed: {}", e))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(timeout_secs)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    Ok(stream)
}

pub fn ensure_ollama_running() -> Result<&'static str, String> {
    if connect_ollama(3).is_ok() {
        return Ok("running");
    }

    let mut cmd = Command::new("ollama");
    cmd.arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x00000008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to start 'ollama serve': {}", e))?;

    for _ in 0..20 {
        thread::sleep(Duration::from_millis(400));
        if connect_ollama(2).is_ok() {
            return Ok("started");
        }
    }
    Err("Timed out waiting for 'ollama serve' on 127.0.0.1:11434.".to_string())
}

pub fn http_json(method: &str, path: &str, body: Option<&Json>, timeout_secs: u64) -> Result<Json, String> {
    let (host, port) = ollama_host_port();
    let mut stream = connect_ollama(timeout_secs)?;
    let payload = body.map(|b| b.encode()).unwrap_or_default();

    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        method, path, host, port
    );
    if !payload.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    req.push_str("\r\n");

    stream
        .write_all(req.as_bytes())
        .and_then(|_| {
            if !payload.is_empty() {
                stream.write_all(payload.as_bytes())
            } else {
                Ok(())
            }
        })
        .map_err(|e| format!("Write error: {}", e))?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| format!("Read status error: {}", e))?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("Read header error: {}", e))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            if key == "content-length" {
                content_length = val.parse().ok();
            } else if key == "transfer-encoding" && val.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }

    let body_bytes = if chunked {
        read_chunked(&mut reader)?
    } else if let Some(len) = content_length {
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| format!("Read body error: {}", e))?;
        buf
    } else {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .map_err(|e| format!("Read body error: {}", e))?;
        buf
    };

    let text = String::from_utf8(body_bytes).map_err(|e| format!("UTF-8 error: {}", e))?;
    json::parse(&text)
}

fn read_chunked<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        reader
            .read_line(&mut size_line)
            .map_err(|e| format!("Chunk size read error: {}", e))?;
        let hex = size_line.trim().split(';').next().unwrap_or("");
        let size = usize::from_str_radix(hex, 16)
            .map_err(|_| format!("Invalid chunk size: '{}'", size_line.trim()))?;
        if size == 0 {
            let mut trailer = String::new();
            let _ = reader.read_line(&mut trailer);
            break;
        }
        let mut chunk = vec![0u8; size];
        reader
            .read_exact(&mut chunk)
            .map_err(|e| format!("Chunk read error: {}", e))?;
        out.extend_from_slice(&chunk);
        let mut crlf = [0u8; 2];
        let _ = reader.read_exact(&mut crlf);
    }
    Ok(out)
}
