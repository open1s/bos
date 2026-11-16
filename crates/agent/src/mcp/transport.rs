use std::process::Stdio;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::error::AgentError;

/// Longest line accepted from an MCP server, in bytes.
const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

fn trim_newline_suffix(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
}

/// Read one newline-terminated line, refusing to buffer more than \`max\` bytes.
///
/// The cap is enforced while reading, so a server that never sends a newline
/// cannot grow the buffer without bound before the caller notices.
async fn read_capped_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut String,
    max: usize,
) -> Result<(), TransportError> {
    line.clear();
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|e| TransportError::Io(e.to_string()))?;
        if available.is_empty() {
            break; // EOF
        }
        let position = available.iter().position(|byte| *byte == b'\n');
        let take = position.map_or(available.len(), |index| index + 1);
        if bytes.len() + take > max {
            return Err(TransportError::Io(format!(
                "Line too large: exceeds {max} bytes"
            )));
        }
        let complete = position.is_some();
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if complete {
            break;
        }
    }
    *line = String::from_utf8(bytes)
        .map_err(|e| TransportError::Io(format!("Line is not valid UTF-8: {e}")))?;
    Ok(())
}

/// Talks to an MCP server over a child process stdio pipes.
pub struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// Errors from the stdio transport.
#[derive(Debug, Clone)]
pub enum TransportError {
    /// Reading or writing a pipe failed.
    Io(String),
    /// The server process could not be spawned.
    Process(String),
    /// The child has no stdout pipe.
    NoStdout,
    /// The child has no stdin pipe.
    NoStdin,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "IO error: {}", s),
            Self::Process(s) => write!(f, "Process error: {}", s),
            Self::NoStdout => write!(f, "No stdout"),
            Self::NoStdin => write!(f, "No stdin"),
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
    /// Spawn `command` with `args` and connect to its stdio.
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, TransportError> {
        Self::spawn_with(command, args, &[], None).await
    }

    /// Spawn a server with an environment and working directory.
    ///
    /// Plugin-declared MCP servers resolve relative commands and args against
    /// their own package root and may carry plugin-specific environment.
    pub async fn spawn_with(
        command: &str,
        args: &[&str],
        env: &[(&str, &str)],
        cwd: Option<&std::path::Path>,
    ) -> Result<Self, TransportError> {
        let mut process = tokio::process::Command::new(command);
        process
            .args(args)
            .envs(env.iter().map(|(key, value)| (*key, *value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let mut child = process
            .spawn()
            .map_err(|e| TransportError::Process(e.to_string()))?;

        let stdin = child.stdin.take().ok_or(TransportError::NoStdin)?;
        let stdout = child.stdout.take().ok_or(TransportError::NoStdout)?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    /// Write one message followed by a newline.
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

    /// Read one newline-terminated line into `buffer`, refusing lines over
    /// 16 MiB while they are still being read.
    pub async fn recv_line(&mut self, buffer: &mut String) -> Result<(), TransportError> {
        read_capped_line(&mut self.stdout, buffer, MAX_LINE_BYTES).await?;
        trim_newline_suffix(buffer);
        Ok(())
    }

    /// Kill the server process.
    pub async fn shutdown(mut self) -> Result<(), TransportError> {
        let _ = self.child.kill().await;
        Ok(())
    }

    /// OS process id of the server, or 0 when unavailable.
    pub fn id(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_a_line_and_leaves_the_next_one() {
        let mut reader = BufReader::new(&b"first\nsecond\n"[..]);
        let mut line = String::new();

        read_capped_line(&mut reader, &mut line, 64).await.unwrap();
        assert_eq!(line, "first\n");

        read_capped_line(&mut reader, &mut line, 64).await.unwrap();
        assert_eq!(line, "second\n");
    }

    /// The cap rejects an oversized line before it is fully buffered.
    #[tokio::test]
    async fn a_line_past_the_cap_is_refused() {
        let payload = "x".repeat(32);
        let mut reader = BufReader::new(payload.as_bytes());
        let mut line = String::new();

        let error = read_capped_line(&mut reader, &mut line, 16)
            .await
            .expect_err("capped read refuses the line");
        assert!(error.to_string().contains("Line too large"), "{error}");
    }

    #[test]
    fn newline_suffixes_are_trimmed() {
        let mut lf = "value\n".to_string();
        trim_newline_suffix(&mut lf);
        assert_eq!(lf, "value");

        let mut crlf = "value\r\n".to_string();
        trim_newline_suffix(&mut crlf);
        assert_eq!(crlf, "value");
    }

    #[test]
    fn test_transport_error_display() {
        let err = TransportError::Io("test error".to_string());
        assert_eq!(err.to_string(), "IO error: test error");

        let err = TransportError::NoStdout;
        assert_eq!(err.to_string(), "No stdout");
    }
}
