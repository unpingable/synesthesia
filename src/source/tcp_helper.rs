use std::{
    io::{BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use crate::source::{
    ebpf_prerequisites::{SUPPORTED_ARCHITECTURE, TCP_COLLECTOR},
    tcp::TcpSourceError,
    tcp_ipc::{NormalizedTcpPulse, TcpWireError},
};

pub struct TcpHelper {
    child: Option<Child>,
    output: BufReader<std::process::ChildStdout>,
    pending: Option<NormalizedTcpPulse>,
}

impl TcpHelper {
    pub fn spawn() -> Result<Self, TcpSourceError> {
        if !cfg!(target_os = "linux") {
            return Err(TcpSourceError::UnsupportedOperatingSystem);
        }
        if std::env::consts::ARCH != SUPPORTED_ARCHITECTURE {
            return Err(TcpSourceError::UnsupportedArchitecture(
                std::env::consts::ARCH.to_owned(),
            ));
        }
        let path = helper_path()?;
        let mut child = Command::new(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    TcpSourceError::MissingDevelopmentDependency(format!(
                        "{} is missing; build or install the Linux TCP helper with the `ebpf` feature",
                        path.display()
                    ))
                } else {
                    TcpSourceError::Load(format!(
                        "could not start {}: {error}",
                        path.display()
                    ))
                }
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            TcpSourceError::Load("TCP collector stdout pipe is unavailable".to_owned())
        })?;
        let mut helper = Self {
            child: Some(child),
            output: BufReader::with_capacity(128 * 1024, stdout),
            pending: None,
        };
        helper.pending = Some(helper.read_pulse()?);
        Ok(helper)
    }

    pub fn next_pulse(&mut self) -> Result<Option<NormalizedTcpPulse>, TcpSourceError> {
        if let Some(pulse) = self.pending.take() {
            return Ok(Some(pulse));
        }
        self.read_pulse().map(Some)
    }

    fn read_pulse(&mut self) -> Result<NormalizedTcpPulse, TcpSourceError> {
        match NormalizedTcpPulse::read_from(&mut self.output) {
            Ok(Some(pulse)) => Ok(pulse),
            Ok(None) => Err(self.exit_error()),
            Err(TcpWireError::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                Err(self.exit_error())
            }
            Err(error) => Err(TcpSourceError::Load(format!(
                "TCP collector protocol failed: {error}"
            ))),
        }
    }

    fn exit_error(&mut self) -> TcpSourceError {
        let Some(mut child) = self.child.take() else {
            return TcpSourceError::Load("TCP collector stopped".to_owned());
        };
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let status = child.wait().ok();
        let detail = stderr.trim();
        if !detail.is_empty() {
            TcpSourceError::classify_load_message(detail)
        } else {
            TcpSourceError::Load(format!(
                "TCP collector exited unexpectedly{}",
                status.map_or_else(String::new, |status| format!(" with {status}"))
            ))
        }
    }
}

impl Drop for TcpHelper {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn helper_path() -> Result<PathBuf, TcpSourceError> {
    let executable = std::env::current_exe().map_err(|error| {
        TcpSourceError::Load(format!("cannot locate the running executable: {error}"))
    })?;
    let directory = executable.parent().ok_or_else(|| {
        TcpSourceError::Load("running executable has no parent directory".to_owned())
    })?;
    let path = directory.join(TCP_COLLECTOR);
    if is_executable_file(&path) {
        Ok(path)
    } else {
        Err(TcpSourceError::MissingDevelopmentDependency(format!(
            "{} is missing; build with `cargo build --release --features ebpf --bins`",
            path.display()
        )))
    }
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_helper_name_is_fixed_not_user_selected() {
        let fake = PathBuf::from("/tmp/bin/synesthesia");
        assert_eq!(
            fake.parent()
                .unwrap()
                .join("synesthesia-tcp-collector")
                .file_name()
                .unwrap(),
            "synesthesia-tcp-collector"
        );
    }

    #[test]
    fn ordinary_files_are_not_accepted_as_tcp_helpers() {
        let path = std::env::temp_dir().join(format!(
            "synesthesia-tcp-helper-test-{}",
            std::process::id()
        ));
        std::fs::File::create(&path).unwrap();
        assert!(!is_executable_file(&path));
        std::fs::remove_file(path).unwrap();
    }
}
