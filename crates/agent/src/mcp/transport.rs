use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::error::AgentError;

fn trim_newline_suffix(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
}

pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    #[allow(dead_code)]
    stderr: tokio::process::ChildStderr,
}

#[derive(Debug, Clone)]
pub enum TransportError {
    Io(String),
    Process(String),
    NoStdout,
    NoStdin,
    NoStderr,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "IO error: {}", s),
            Self::Process(s) => write!(f, "Process error: {}", s),
            Self::NoStdout => write!(f, "No stdout"),
            Self::NoStdin => write!(f, "No stdin"),
            Self::NoStderr => write!(f, "No stderr"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<TransportError> for AgentError {
    fn from(e: TransportError) -> Self {
        AgentError::Session(e.to_string())
    }
}

impl StdioTransport {
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, TransportError> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| TransportError::Process(e.to_string()))?;

        let stdin = child.stdin.take().ok_or(TransportError::NoStdin)?;
        let stdout = child.stdout.take().ok_or(TransportError::NoStdout)?;
        let stderr = child.stderr.take().ok_or(TransportError::NoStderr)?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr,
        })
    }

    pub async fn send(&mut self, msg: &[u8]) -> Result<(), TransportError> {
        self.stdin
            .write_all(msg)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        self.stdin
            .write_all(b"\n")
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(())
    }

    pub async fn recv_line(&mut self, buffer: &mut String) -> Result<(), TransportError> {
        buffer.clear();
        let bytes_read = self.stdout
            .read_line(buffer)
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        // Cap line size at 16MB to prevent memory exhaustion
        if bytes_read > 16 * 1024 * 1024 {
            return Err(TransportError::Io(format!(
                "Line too large: {bytes_read} bytes (max 16MB)"
            )));
        }
        trim_newline_suffix(buffer);
        Ok(())
    }

    pub async fn recv_line_with_capacity(
        &mut self,
        buffer: &mut String,
        min_capacity: usize,
    ) -> Result<(), TransportError> {
        if buffer.capacity() < min_capacity {
            buffer.reserve(min_capacity - buffer.capacity());
        }
        self.recv_line(buffer).await
    }

    pub async fn shutdown(mut self) -> Result<(), TransportError> {
        let _ = self.child.kill().await;
        Ok(())
    }

    pub fn id(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_error_display() {
        let err = TransportError::Io("test error".to_string());
        assert_eq!(err.to_string(), "IO error: test error");

        let err = TransportError::NoStdout;
        assert_eq!(err.to_string(), "No stdout");
    }
}
