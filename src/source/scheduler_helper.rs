use std::{
    io::{BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use crate::source::{
    scheduler::SchedulerSourceError,
    scheduler_ipc::{NormalizedSchedulerPulse, SchedulerWireError},
};

pub struct SchedulerHelper {
    child: Option<Child>,
    output: BufReader<std::process::ChildStdout>,
    pending: Option<NormalizedSchedulerPulse>,
}

impl SchedulerHelper {
    pub fn spawn() -> Result<Self, SchedulerSourceError> {
        if !cfg!(target_os = "linux") {
            return Err(SchedulerSourceError::UnsupportedOperatingSystem);
        }
        let path = helper_path()?;
        let mut child = Command::new(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    SchedulerSourceError::MissingDevelopmentDependency(format!(
                        "{} is missing; build or install the Linux helper with the `ebpf` feature",
                        path.display()
                    ))
                } else {
                    SchedulerSourceError::Load(format!(
                        "could not start {}: {error}",
                        path.display()
                    ))
                }
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SchedulerSourceError::Load("scheduler helper stdout pipe is unavailable".to_owned())
        })?;
        let mut helper = Self {
            child: Some(child),
            output: BufReader::with_capacity(128 * 1024, stdout),
            pending: None,
        };
        helper.pending = Some(helper.read_pulse()?);
        Ok(helper)
    }

    pub fn next_pulse(&mut self) -> Result<Option<NormalizedSchedulerPulse>, SchedulerSourceError> {
        if let Some(pulse) = self.pending.take() {
            return Ok(Some(pulse));
        }
        self.read_pulse().map(Some)
    }

    fn read_pulse(&mut self) -> Result<NormalizedSchedulerPulse, SchedulerSourceError> {
        match NormalizedSchedulerPulse::read_from(&mut self.output) {
            Ok(Some(pulse)) => Ok(pulse),
            Ok(None) => Err(self.exit_error()),
            Err(SchedulerWireError::Io(error))
                if error.kind() == std::io::ErrorKind::BrokenPipe =>
            {
                Err(self.exit_error())
            }
            Err(error) => Err(SchedulerSourceError::Load(format!(
                "scheduler helper protocol failed: {error}"
            ))),
        }
    }

    fn exit_error(&mut self) -> SchedulerSourceError {
        let Some(mut child) = self.child.take() else {
            return SchedulerSourceError::Load("scheduler helper stopped".to_owned());
        };
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let status = child.wait().ok();
        let detail = stderr.trim();
        if !detail.is_empty() {
            SchedulerSourceError::classify_load_message(detail)
        } else {
            SchedulerSourceError::Load(format!(
                "scheduler helper exited unexpectedly{}",
                status.map_or_else(String::new, |status| format!(" with {status}"))
            ))
        }
    }
}

impl Drop for SchedulerHelper {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn helper_path() -> Result<PathBuf, SchedulerSourceError> {
    let executable = std::env::current_exe().map_err(|error| {
        SchedulerSourceError::Load(format!("cannot locate the running executable: {error}"))
    })?;
    let directory = executable.parent().ok_or_else(|| {
        SchedulerSourceError::Load("running executable has no parent directory".to_owned())
    })?;
    let path = directory.join("synesthesia-scheduler-collector");
    if is_executable_file(&path) {
        Ok(path)
    } else {
        Err(SchedulerSourceError::MissingDevelopmentDependency(format!(
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
    fn helper_name_is_fixed_not_user_selected() {
        let fake = PathBuf::from("/tmp/bin/synesthesia");
        assert_eq!(
            fake.parent()
                .unwrap()
                .join("synesthesia-scheduler-collector")
                .file_name()
                .unwrap(),
            "synesthesia-scheduler-collector"
        );
    }

    #[test]
    fn ordinary_files_are_not_accepted_as_helpers() {
        let path =
            std::env::temp_dir().join(format!("synesthesia-helper-test-{}", std::process::id()));
        std::fs::File::create(&path).unwrap();
        assert!(!is_executable_file(&path));
        std::fs::remove_file(path).unwrap();
    }
}
