//! The original's `urllib.request` calls, made with `curl`: reading a URL
//! (the AUR's RPC, the Arch Wiki's API, the Arch Linux Archive) and saving one
//! to a file. Failures are worded like urllib's (`HTTP Error 404: Not Found`,
//! `<urlopen error ...>`).

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// `urlopen(url, timeout=seconds).read()`. The timeout is, as urllib's, how
/// long the connection may stay silent, not how long the whole read may take.
/// Blocking.
pub fn get(url: &str, timeout: u32) -> Result<Vec<u8>, String> {
    let timeout = timeout.to_string();
    let mut command = Command::new("curl");
    command.args([
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--connect-timeout",
        &timeout,
        "--speed-time",
        &timeout,
        "--speed-limit",
        "1",
        "--write-out",
        "\n%{http_code}",
        url,
    ]);
    let (status, mut body, said) = run(command)?;
    // The status code curl wrote after the body.
    let code = body
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map(|newline| {
            let code = String::from_utf8_lossy(&body[newline + 1..])
                .trim()
                .to_owned();
            body.truncate(newline);
            code
        })
        .unwrap_or_default();
    if status == Some(0) {
        return Ok(body);
    }
    Err(failure(status, &code, &said))
}

/// `urlretrieve(url, path)`. Blocking.
pub fn download(url: &str, path: &Path) -> Result<(), String> {
    let mut command = Command::new("curl");
    command
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--write-out",
            "%{http_code}",
            "--output",
        ])
        .arg(path)
        .arg(url);
    let (status, code, said) = run(command)?;
    if status == Some(0) {
        return Ok(());
    }
    Err(failure(status, &String::from_utf8_lossy(&code), &said))
}

/// Run curl: its exit status, what it wrote to stdout and to stderr.
fn run(mut command: Command) -> Result<(Option<i32>, Vec<u8>, String), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("<urlopen error {error}>"))?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let reader = std::thread::spawn(move || {
        let mut said = String::new();
        let _ = stderr.read_to_string(&mut said);
        said
    });
    let mut body = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut body);
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    let said = reader.join().unwrap_or_default();
    Ok((status.code(), body, said))
}

fn failure(status: Option<i32>, code: &str, said: &str) -> String {
    let http = code.trim().parse::<u16>().unwrap_or(0);
    if status == Some(22) && http >= 400 {
        return format!("HTTP Error {http}: {}", reason(http));
    }
    let said = said
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("curl: "))
        .map(|line| match line.split_once(") ") {
            Some((number, rest)) if number.starts_with('(') => rest.to_owned(),
            _ => line.to_owned(),
        })
        .unwrap_or_else(|| match status {
            Some(code) => format!("curl exited with status {code}"),
            None => "curl was killed".into(),
        });
    format!("<urlopen error {said}>")
}

/// `http.client.responses`, for the codes these servers answer.
fn reason(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        410 => "Gone",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// `urllib.parse.quote(text)`: UTF-8 percent-encoding of everything but
/// letters, digits, `_.-~` and `/`.
pub fn quote(text: &str) -> String {
    let mut quoted = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~/".contains(&byte) {
            quoted.push(byte as char);
        } else {
            quoted.push_str(&format!("%{byte:02X}"));
        }
    }
    quoted
}

/// `urllib.parse.unquote(text)`: `%XX` decoded as UTF-8 (invalid sequences
/// become U+FFFD).
pub fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(byte) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_matches_urllib() {
        assert_eq!(quote("nvidia-open"), "nvidia-open");
        assert_eq!(quote("Visual Studio Code"), "Visual%20Studio%20Code");
        assert_eq!(quote("a+b&c=d/é"), "a%2Bb%26c%3Dd/%C3%A9");
        assert_eq!(unquote("gtk%2B3-1%3A3.24-1"), "gtk+3-1:3.24-1");
        assert_eq!(unquote("100%"), "100%");
        assert_eq!(unquote("%zz%41"), "%zzA");
    }

    #[test]
    fn failures_read_like_urllib() {
        assert_eq!(failure(Some(22), "404", ""), "HTTP Error 404: Not Found");
        assert_eq!(
            failure(
                Some(6),
                "000",
                "curl: (6) Could not resolve host: aur.archlinux.org\n"
            ),
            "<urlopen error Could not resolve host: aur.archlinux.org>"
        );
    }
}
