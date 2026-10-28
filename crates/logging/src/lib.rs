//! Logging setup for BrainOS.
//!
//! Provides the log-file formatter, the rotating logger initializer, and a
//! small test helper.
#![warn(missing_docs)]

use flexi_logger::{Cleanup, Criterion, DeferredNow, FileSpec, Logger, Naming};
use log::Record;
use std::{
    cell::RefCell,
    fmt,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

static LOGGER_HANDLE: Mutex<Option<flexi_logger::LoggerHandle>> = Mutex::new(None);

/// Flexi-logger format function: one compact line per record, for log files.
pub fn short_format(
    w: &mut dyn Write,
    now: &mut DeferredNow,
    record: &Record,
) -> std::io::Result<()> {
    let file = record.file().unwrap_or("unknown");

    let short_file = Path::new(file)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(file);

    write!(
        w,
        "[{}] {} [{}] {}:{} {}",
        now.now().format("%Y-%m-%d %H:%M:%S"),
        record.level(),
        record.module_path().unwrap_or(""),
        short_file,
        record.line().unwrap_or(0),
        record.args()
    )
}

thread_local! {
    /// Reused so the stdout formatter does not allocate a fresh String for
    /// every log record.
    static MESSAGE_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
}

fn pretty_stdout(w: &mut dyn Write, now: &mut DeferredNow, record: &Record) -> std::io::Result<()> {
    let ts = now.now().format("%H:%M:%S%.3f");
    let level = record.level();
    let file = record.file().unwrap_or("unknown");
    let line = record.line().unwrap_or(0);

    writeln!(w, "🕒 {} │ {:<5} │ {}:{}", ts, level, file, line)?;
    write_pretty_message(w, record.args())
}

/// Cheap prefix check that keeps most ordinary log lines out of the JSON
/// parser; only values that could begin valid JSON are attempted.
fn looks_like_json(message: &str) -> bool {
    matches!(
        message.trim_start().as_bytes().first(),
        Some(b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n')
    )
}

fn write_pretty_message(w: &mut dyn Write, args: &fmt::Arguments<'_>) -> std::io::Result<()> {
    use std::fmt::Write as _;

    MESSAGE_BUFFER
        .try_with(|cell| {
            let Ok(mut buffer) = cell.try_borrow_mut() else {
                // Recursive logging: format directly instead of panicking on a
                // re-entrant borrow of the shared buffer.
                return write!(w, "{}", args);
            };
            buffer.clear();
            // Do not retain a huge buffer for the life of the thread just
            // because one unusually large record was logged.
            if buffer.capacity() > 64 * 1024 {
                buffer.shrink_to(4096);
            }
            // Writing to a String cannot fail.
            let _ = write!(buffer, "{}", args);

            let trimmed = buffer.trim_start();
            if looks_like_json(trimmed) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                    if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                        // flexi_logger appends the record's trailing newline.
                        return write!(w, "📦 JSON:\n{}", pretty);
                    }
                }
            }
            write!(w, "{}", buffer)
        })
        .unwrap_or_else(|_| write!(w, "{}", args))
}

/// The default log directory: `~/.bos/log`. `None` when no home directory
/// can be determined.
pub fn default_log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|d| d.join(".bos").join("log"))
}

/// Initialize the global logger.
///
/// Writes rotating files under [`default_log_dir`] with the basename `bos`
/// and mirrors them to stdout. The level comes from `RUST_LOG`, then
/// `BOS_LOG`, then the `logging.level` config key, defaulting to `error`;
/// an unrecognized level falls back to `info`.
/// Returns early if the log directory cannot be created; a failure to
/// install the logger is printed to stderr.
pub fn auto_init_tracing() {
    if let Some(dir) = default_log_dir() {
        auto_init_tracing_in(dir, "bos");
    }
}

/// Initialize the global logger into a caller-chosen directory and file
/// basename.
///
/// `log_dir` is created if missing; rotating files carry `basename` (e.g.
/// `basename("gui")` writes `gui.log`, keeping rotated files beside it).
/// Level resolution, rotation policy and stdout mirroring match
/// [`auto_init_tracing`].
pub fn auto_init_tracing_in(log_dir: impl AsRef<Path>, basename: &str) {
    let logdir = log_dir.as_ref();

    if std::fs::create_dir_all(logdir).is_err() {
        return;
    }

    let level = std::env::var("RUST_LOG")
        .or_else(|_| std::env::var("BOS_LOG"))
        .unwrap_or_else(|_| {
            let mut loader = config::loader::ConfigLoader::new().discover();
            match loader.load_sync() {
                Ok(config) => config
                    .get("logging")
                    .and_then(|l| l.get("level"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("error")
                    .to_string(),
                Err(_) => "error".to_string(),
            }
        });

    let valid_levels = ["error", "warn", "info", "debug", "trace"];
    let level = if valid_levels.contains(&level.as_str()) {
        level
    } else {
        "info".to_string()
    };

    // `ui` is the GUI's decision trail (webview lines and backend choices
    // alike); it stays at info so the trail survives an error-level default,
    // and matches no target in non-GUI processes.
    let level = format!(
        "bus={level},agent={level},react={level},nbos={level},resource={level},ui=info,zenoh=off,h2=off,rustls=off"
    );

    let file_spec = FileSpec::default().directory(logdir).basename(basename);
    let logger = match Logger::try_with_str(level) {
        Ok(logger) => logger,
        Err(e) => {
            eprintln!("logging: invalid level filter: {}", e);
            return;
        }
    };
    let logger = logger
        .log_to_file(file_spec)
        // Append, not truncate: a plain start renames the previous run's file
        // and begins a new one, so a reader who looks for "the log" after a
        // restart finds only the last launch. Rotation still bounds the set.
        .append()
        .rotate(
            Criterion::Size(10_000_000), // 10 MB
            Naming::TimestampsCustomFormat {
                current_infix: Some(""),
                format: "%Y-%m-%dH%H-%M-%S",
            },
            Cleanup::KeepLogFiles(10), // 保留 10 个文件
        )
        .duplicate_to_stdout(flexi_logger::Duplicate::All) // 同步输出到控制台
        .format_for_files(short_format) // 文件日志详细
        .format_for_stdout(pretty_stdout);

    match logger.start() {
        Ok(handle) => {
            if let Ok(mut guard) = LOGGER_HANDLE.lock() {
                *guard = Some(handle);
            }
        }
        Err(e) => {
            eprintln!("logging: failed to init logger: {}", e);
        }
    }
}

/// Log `message` at info level with a `[TEST]` prefix.
pub fn log_test_message(message: &str) {
    log::info!("[TEST] {}", message);
}

#[cfg(test)]
mod tests {
    use config::loader::ConfigLoader;

    #[test]
    fn it_works() {
        let logdir = dirs::home_dir().unwrap().join(".bos/log");
        println!("{:?}", logdir);
    }

    #[test]
    fn test_load_log_level_from_config() {
        let mut loader = ConfigLoader::new().discover();
        let config = loader.load_sync().unwrap();
        let level = config
            .get("logging")
            .and_then(|l| l.get("level"))
            .and_then(|v| v.as_str())
            .unwrap_or("error");
        assert!(
            ["trace", "debug", "info", "warn", "error", "off"].contains(&level),
            "unexpected log level: {}",
            level
        );
    }

    #[test]
    fn test_auto_init_tracing_in_writes_to_given_dir_and_basename() {
        use crate::auto_init_tracing_in;

        // Own the path this test asserts about: a per-process temp dir,
        // cleared first so a leftover file from an earlier run cannot make
        // the assertion pass for the wrong reason.
        let dir = std::env::temp_dir().join(format!("bos-logging-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        auto_init_tracing_in(&dir, "test-basename");
        // Target `ui` on purpose: it is the one target the filter always
        // keeps at info, and the line only lands if that directive works.
        log::info!(target: "ui", "DIRECT LOG TEST");

        // The logger may write on its own thread; poll briefly for the line.
        let file = dir.join("test-basename.log");
        let mut content = String::new();
        for _ in 0..50 {
            if let Ok(c) = std::fs::read_to_string(&file) {
                if c.contains("DIRECT LOG TEST") {
                    content = c;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        assert!(
            content.contains("DIRECT LOG TEST"),
            "expected {} to hold the logged line",
            file.display()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_pretty_stdout_keeps_plain_messages_intact() {
        let mut out = Vec::new();
        let mut now = flexi_logger::DeferredNow::new();
        let record = log::Record::builder()
            .args(format_args!("plain message"))
            .level(log::Level::Info)
            .target("ui")
            .file(Some("src/lib.rs"))
            .line(Some(1))
            .build();

        crate::pretty_stdout(&mut out, &mut now, &record).expect("formatting");

        let text = String::from_utf8(out).expect("formatter output is utf-8");
        assert!(text.contains("plain message"), "{text}");
        assert!(!text.contains("📦 JSON"), "{text}");
        assert_eq!(
            text.matches('\n').count(),
            1,
            "exactly one metadata/message separator: {text:?}"
        );
        assert!(
            text.ends_with("plain message"),
            "the logger appends the trailing newline: {text:?}"
        );
    }

    #[test]
    fn test_pretty_stdout_pretty_prints_json() {
        let mut out = Vec::new();
        let mut now = flexi_logger::DeferredNow::new();
        let record = log::Record::builder()
            .args(format_args!("{{\"a\":1,\"b\":[true,null]}}"))
            .level(log::Level::Info)
            .target("ui")
            .build();

        crate::pretty_stdout(&mut out, &mut now, &record).expect("formatting");

        let text = String::from_utf8(out).expect("formatter output is utf-8");
        assert!(text.contains("📦 JSON:"), "{text}");
        assert!(text.contains("\"a\": 1"), "{text}");
    }
}
