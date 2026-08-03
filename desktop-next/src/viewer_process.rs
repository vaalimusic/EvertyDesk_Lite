use crate::ipc::MAX_IPC_LINE_BYTES;
use crate::protocol::{ViewerBootstrap, ViewerCommand};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub const VIEWER_PATH_ENV: &str = "EVERTYDESK_VIEWER_PATH";
const CONTROL_QUEUE_CAPACITY: usize = 32;

pub fn viewer_executable_path() -> io::Result<PathBuf> {
    if let Some(configured) = std::env::var_os(VIEWER_PATH_ENV) {
        if !configured.is_empty() {
            return Ok(PathBuf::from(configured));
        }
    }

    let current = std::env::current_exe()?;
    let directory = current.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "launcher executable has no parent directory",
        )
    })?;
    Ok(directory.join(viewer_filename()))
}

pub struct ViewerProcess {
    child: Child,
    control: SyncSender<Vec<u8>>,
    control_open: Arc<AtomicBool>,
}

impl ViewerProcess {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        wait_for_child_exit(&mut self.child, timeout)
    }

    pub fn send(&mut self, command: ViewerCommand) -> io::Result<()> {
        let encoded = serde_json::to_vec(&command).map_err(io::Error::other)?;
        ensure_ipc_size(&encoded)?;
        enqueue_control(&self.control, &self.control_open, encoded)
    }

    pub fn disconnect(&mut self) -> io::Result<()> {
        self.send(ViewerCommand::Disconnect)
    }

    fn shutdown(&mut self) {
        if matches!(self.wait_for_exit(Duration::ZERO), Ok(Some(_))) {
            return;
        }
        let _ = self.disconnect();
        if matches!(self.wait_for_exit(Duration::from_millis(500)), Ok(Some(_))) {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ViewerProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn spawn_viewer(request: &ViewerBootstrap) -> io::Result<ViewerProcess> {
    request
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

    let executable = viewer_executable_path()?;
    spawn_viewer_at(&executable, request)
}

fn spawn_viewer_at(executable: &Path, request: &ViewerBootstrap) -> io::Result<ViewerProcess> {
    let encoded = Zeroizing::new(serde_json::to_vec(request).map_err(io::Error::other)?);
    ensure_ipc_size(&encoded)?;

    let mut child = Command::new(executable)
        .arg("--bootstrap-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("could not start {}: {error}", executable.display()),
            )
        })?;

    let mut control = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "viewer stdin unavailable"))?;
    let write_result = control
        .write_all(&encoded)
        .and_then(|()| control.write_all(b"\n"))
        .and_then(|()| control.flush());

    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

    let process_id = child.id();
    let (control, control_open) = match start_control_writer(process_id, control) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };

    Ok(ViewerProcess {
        child,
        control,
        control_open,
    })
}

fn start_control_writer(
    process_id: u32,
    mut control: ChildStdin,
) -> io::Result<(SyncSender<Vec<u8>>, Arc<AtomicBool>)> {
    let (commands, receiver) = mpsc::sync_channel::<Vec<u8>>(CONTROL_QUEUE_CAPACITY);
    let open = Arc::new(AtomicBool::new(true));
    let writer_open = Arc::clone(&open);
    thread::Builder::new()
        .name(format!("viewer-control-writer-{process_id}"))
        .spawn(move || {
            while let Ok(encoded) = receiver.recv() {
                let result = control
                    .write_all(&encoded)
                    .and_then(|()| control.write_all(b"\n"))
                    .and_then(|()| control.flush());
                if result.is_err() {
                    break;
                }
            }
            writer_open.store(false, Ordering::Release);
        })?;
    Ok((commands, open))
}

fn enqueue_control(
    commands: &SyncSender<Vec<u8>>,
    open: &AtomicBool,
    encoded: Vec<u8>,
) -> io::Result<()> {
    if !open.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "viewer control channel is closed",
        ));
    }
    commands.try_send(encoded).map_err(|error| match error {
        TrySendError::Full(_) => {
            io::Error::new(io::ErrorKind::WouldBlock, "viewer control queue is full")
        }
        TrySendError::Disconnected(_) => io::Error::new(
            io::ErrorKind::BrokenPipe,
            "viewer control writer is unavailable",
        ),
    })
}

fn wait_for_child_exit(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn ensure_ipc_size(encoded: &[u8]) -> io::Result<()> {
    if encoded.len() > MAX_IPC_LINE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "IPC message is {} bytes; limit is {MAX_IPC_LINE_BYTES}",
                encoded.len()
            ),
        ));
    }
    Ok(())
}

fn viewer_filename() -> OsString {
    let mut name = OsString::from("evertydesk-viewer");
    name.push(std::env::consts::EXE_SUFFIX);
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_writer_accepts_the_limit_and_rejects_larger_messages() {
        assert!(ensure_ipc_size(&vec![0; MAX_IPC_LINE_BYTES]).is_ok());
        let error = ensure_ipc_size(&vec![0; MAX_IPC_LINE_BYTES + 1]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn bounded_control_queue_reports_backpressure_and_disconnect() {
        let (commands, receiver) = mpsc::sync_channel(1);
        let open = AtomicBool::new(true);

        enqueue_control(&commands, &open, b"first".to_vec()).unwrap();
        let full = enqueue_control(&commands, &open, b"second".to_vec()).unwrap_err();
        assert_eq!(full.kind(), io::ErrorKind::WouldBlock);

        drop(receiver);
        let closed = enqueue_control(&commands, &open, b"third".to_vec()).unwrap_err();
        assert_eq!(closed.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn closed_control_flag_fails_without_touching_the_queue() {
        let (commands, _receiver) = mpsc::sync_channel(1);
        let open = AtomicBool::new(false);
        let error = enqueue_control(&commands, &open, Vec::new()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[cfg(windows)]
    #[test]
    fn wait_for_exit_reaps_a_completed_viewer_process() {
        let mut child = Command::new("cmd")
            .args(["/C", "exit", "0"])
            .spawn()
            .unwrap();

        let status = wait_for_child_exit(&mut child, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(status.success());
    }
}
