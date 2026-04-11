use flexi_logger::{Cleanup, Criterion, DeferredNow, FileSpec, Logger, Naming};
use log::Record;
use std::{io::Write, path::Path};

pub fn short_format(
    w: &mut dyn Write,
    now: &mut DeferredNow,
    record: &Record,
) -> std::io::Result<()> {
    let file = record.file().unwrap_or("unknown");

    // 👇 核心：只取文件名
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
    // 👉 如果是 JSON，直接美化
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(msg) {
        return format!(
            "{}\n{}",
            "📦 JSON:",
            serde_json::to_string_pretty(&json).unwrap()
        );
    }

    // 👉 Agent log 特化（你的场景）
    // if msg.contains("LlmRequest") {
    //     return msg
    //         .replace("messages:", "\n📨 messages:\n")
    //         .replace("System", "\n  🧠 System")
    //         .replace("User", "\n  👤 User")
    //         .replace("AssistantToolCall", "\n  🔧 ToolCall")
    //         .replace("ToolResult", "\n  ✅ ToolResult");
    // }

    msg.to_string()
}

pub fn auto_init_tracing() {
    let logdir = dirs::home_dir().unwrap().join(".bos/log");
    let file_spec = FileSpec::default().directory(logdir).basename("bos");
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

    let level = level.to_lowercase();

    let valid_levels = ["error", "warn", "info", "debug", "trace"];
    let level = if valid_levels.contains(&level.as_str()) {
        level
    } else {
        "error".to_string()
    };

    let level = format!(
        "bus={level},agent={level},react={level},pybos={level},zenoh=off,h2=off,rustls=off"
    );

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
        // .format_for_stdout(flexi_logger::colored_detailed_format)
        .format_for_stdout(pretty_stdout)
        .start()
        .unwrap();
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
}
