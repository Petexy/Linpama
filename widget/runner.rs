//! Running pacman and friends the way the original did: `Popen` with both
//! streams in one pipe, read a line at a time in text mode (so `\r` ends a
//! line too), with the process kept for the Cancel button.

use linexin_plugin_api::Host;
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

/// How privileged commands are authorized.
#[derive(Clone, Copy)]
pub enum Privilege {
    /// The Center's shared sudo helper, as the Python widget's `sudo_manager`
    /// (GTK: the password was asked for already).
    Sudo(Option<Host>),
    /// `pkexec`, answered by the session's polkit agent — on LineXinBar the
    /// shell's own controller-friendly password panel.
    Polkit,
}

impl Privilege {
    /// `sudo_manager.wrapper_path`.
    pub fn wrapper(&self) -> String {
        match self {
            Privilege::Sudo(Some(host)) => host.sudo_wrapper_path(),
            Privilege::Sudo(None) => "sudo".into(),
            Privilege::Polkit => "pkexec".into(),
        }
    }

    /// `sudo_manager.askpass_script`.
    pub fn askpass(&self) -> Option<String> {
        match self {
            Privilege::Sudo(Some(host)) => Some(host.sudo_askpass_path()),
            _ => None,
        }
    }

    pub fn start(&self) {
        if let Privilege::Sudo(Some(host)) = self {
            host.sudo_start_session();
        }
    }

    pub fn stop(&self) {
        if let Privilege::Sudo(Some(host)) = self {
            host.sudo_stop_session();
        }
    }

    pub fn forget(&self) {
        if let Privilege::Sudo(Some(host)) = self {
            host.sudo_forget_password();
        }
    }

    /// The Center to lock while `argv` runs: the original Center locked its
    /// window for a command through the sudo wrapper.
    fn lock_for(&self, program: &str) -> Option<Host> {
        let Privilege::Sudo(Some(host)) = self else {
            return None;
        };
        let wrapper = host.sudo_wrapper_path();
        let name = Path::new(&wrapper)
            .file_name()?
            .to_string_lossy()
            .into_owned();
        (!wrapper.is_empty() && (program == wrapper || program.ends_with(&name))).then_some(*host)
    }
}

/// The process the Cancel button interrupts (0: none).
#[derive(Clone, Default)]
pub struct Current(Arc<AtomicI32>);

impl Current {
    /// `current_process.send_signal(SIGINT)`; whether there was one.
    pub fn interrupt(&self) -> bool {
        let pid = self.0.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: a plain signal to a process this widget started.
            unsafe { libc::kill(pid, libc::SIGINT) };
            true
        } else {
            false
        }
    }

    pub fn running(&self) -> bool {
        self.0.load(Ordering::SeqCst) > 0
    }

    fn set(&self, pid: i32) {
        self.0.store(pid, Ordering::SeqCst);
    }
}

/// What starting or running a command needs besides its argv.
#[derive(Default)]
pub struct Options<'a> {
    /// `LC_ALL=C` in its environment.
    pub c_locale: bool,
    pub env: Vec<(String, String)>,
    pub cwd: Option<&'a Path>,
}

/// The original's `Popen(argv, stdout=PIPE, stderr=STDOUT, text=True)` and
/// readline loop: every line to `line`, the exit status back. `Err` is the
/// message of the exception the original caught (`str(e)`). Blocking.
pub fn run_logged(
    argv: &[String],
    options: Options,
    privilege: Privilege,
    current: &Current,
    line: &dyn Fn(String),
) -> Result<ExitStatus, String> {
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let (mut reader, writer) = std::io::pipe().map_err(|error| error.to_string())?;
    let error_writer = writer.try_clone().map_err(|error| error.to_string())?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(writer)
        .stderr(error_writer);
    if options.c_locale {
        command.env("LC_ALL", "C");
    }
    command.envs(options.env.iter().map(|(key, value)| (key, value)));
    if let Some(cwd) = options.cwd {
        command.current_dir(cwd);
    }
    let lock = privilege.lock_for(program);
    if let Some(host) = lock {
        host.lock_ui();
    }
    let spawned = command.spawn();
    drop(command);
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            if let Some(host) = lock {
                host.unlock_ui();
            }
            return Err(os_error(&error, program));
        }
    };
    current.set(child.id() as i32);
    read_lines(&mut reader, line);
    let status = child.wait().map_err(|error| error.to_string());
    current.set(0);
    if let Some(host) = lock {
        host.unlock_ui();
    }
    status
}

/// `subprocess.run(argv, capture_output=True, text=True)`: exit status,
/// stdout and stderr (the Center locked while a wrapper command runs).
pub fn run_captured(
    argv: &[String],
    options: Options,
    privilege: Privilege,
) -> Result<(ExitStatus, String, String), String> {
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::inherit());
    if options.c_locale {
        command.env("LC_ALL", "C");
    }
    command.envs(options.env.iter().map(|(key, value)| (key, value)));
    let lock = privilege.lock_for(program);
    if let Some(host) = lock {
        host.lock_ui();
    }
    let output = command.output();
    if let Some(host) = lock {
        host.unlock_ui();
    }
    let output = output.map_err(|error| os_error(&error, program))?;
    Ok((
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// Lines as text mode with universal newlines reads them: `\r`, `\r\n` and
/// `\n` end a line and become `\n`; the last line keeps no ending.
fn read_lines(reader: &mut impl Read, line: &dyn Fn(String)) {
    let mut pending: Vec<u8> = Vec::new();
    let mut after_cr = false;
    let mut buffer = [0u8; 8192];
    let emit = |pending: &mut Vec<u8>| {
        pending.push(b'\n');
        line(String::from_utf8_lossy(pending).into_owned());
        pending.clear();
    };
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &byte in &buffer[..read] {
            match byte {
                b'\n' if after_cr => after_cr = false,
                b'\n' => emit(&mut pending),
                b'\r' => {
                    emit(&mut pending);
                    after_cr = true;
                }
                byte => {
                    after_cr = false;
                    pending.push(byte);
                }
            }
        }
    }
    if !pending.is_empty() {
        line(String::from_utf8_lossy(&pending).into_owned());
    }
}

/// `str(OSError)` for a program that could not start.
pub fn os_error(error: &std::io::Error, program: &str) -> String {
    match error.raw_os_error() {
        Some(code) => {
            // SAFETY: strerror returns a NUL-terminated message for any number.
            let message =
                unsafe { std::ffi::CStr::from_ptr(libc::strerror(code)) }.to_string_lossy();
            format!("[Errno {code}] {message}: '{program}'")
        }
        None => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn lines_arrive_like_text_mode_reads_them() {
        let lines = Mutex::new(Vec::new());
        let current = Current::default();
        let argv: Vec<String> = [
            "sh",
            "-c",
            "printf 'a\\r\\nb\\rc\\n'; echo err >&2; printf end",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        let status = run_logged(
            &argv,
            Options::default(),
            Privilege::Polkit,
            &current,
            &|line| lines.lock().unwrap().push(line),
        )
        .unwrap();
        assert!(status.success());
        assert!(!current.running());
        assert_eq!(
            *lines.lock().unwrap(),
            ["a\n", "b\n", "c\n", "err\n", "end"]
        );
        let missing = vec!["/nonexistent/paru".to_string()];
        assert_eq!(
            run_logged(
                &missing,
                Options::default(),
                Privilege::Polkit,
                &current,
                &|_| {}
            ),
            Err("[Errno 2] No such file or directory: '/nonexistent/paru'".into())
        );
    }
}
