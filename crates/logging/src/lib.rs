use flexi_logger::{Cleanup, Criterion, DeferredNow, FileSpec, Logger, Naming};
use log::Record;
use std::{io::Write, path::Path, sync::Mutex};

static LOGGER_HANDLE: Mutex<Option<flexi_logger::LoggerHandle>> = Mutex::new(None);

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
        &record.args()
    )
}

fn pretty_stdout(w: &mut dyn Write, now: &mut DeferredNow, record: &Record) -> std::io::Result<()> {
    let ts = now.now().format("%H:%M:%S%.3f");
    let level = record.level();
    let file = record.file().unwrap_or("unknown");
    let line = record.line().unwrap_or(0);

    writeln!(
        w,
        "🕒 {} │ {:<5} │ {}:{}\n{}",
        ts,
        level,
        file,
        line,
        pretty_msg(&record.args().to_string())
    )
}

fn pretty_msg(msg: &str) -> String {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(msg) {
        return format!(
            "{}\n{}",
            "📦 JSON:",
            serde_json::to_string_pretty(&json).unwrap()
        );
    }
    msg.to_string()
}

pub fn auto_init_tracing() {
    let logdir = match dirs::home_dir() {
        Some(d) => d.join(".bos/log"),
        None => {
            return;
        }
    };

    if let Err(_) = std::fs::create_dir_all(&logdir) {
        return;
    }

    let level = std::env::var("BOS_LOG");
    let level = level.unwrap_or_else(|_| {
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

    let level = format!(
        "bus={level},agent={level},react={level},pybos={level},zenoh=off,h2=off,rustls=off"
    );
    
    let file_spec = FileSpec::default().directory(logdir).basename("bos");
    let logger = Logger::try_with_str(level)
        .unwrap()
        .log_to_file(file_spec)
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
            ["debug", "info", "warn", "error"].contains(&level),
            "unexpected log level: {}",
            level
        );
    }

    #[test]
    fn test_auto_init_tracing_creates_log_file() {
        use crate::{auto_init_tracing, log_test_message};

        auto_init_tracing();

        log::info!("DIRECT LOG TEST");
        log_test_message("test message from rust test");

        let logdir = dirs::home_dir().unwrap().join(".bos/log");
        println!("Log directory: {:?}", logdir);

        if let Ok(entries) = std::fs::read_dir(&logdir) {
            let files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            println!("Found {} files in log dir", files.len());
            for entry in &files {
                println!("  - {:?}", entry.path());
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    println!(
                        "    Content: {}",
                        content.lines().take(3).collect::<Vec<_>>().join("\n    ")
                    );
                }
            }
        } else {
            println!("Could not read log directory");
        }
    }
}
