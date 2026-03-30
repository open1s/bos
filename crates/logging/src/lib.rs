use std::io::Write;
use flexi_logger::{Logger, Criterion, Naming, Cleanup, FileSpec, DeferredNow};
use log::Record;

fn pretty_stdout(
    w: &mut dyn Write,
    now: &mut DeferredNow,
    record: &Record,
) -> std::io::Result<()> {
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


#[ctor::ctor]
pub fn auto_init_tracing() {
    let file_spec = FileSpec::default()
        .directory("log")
        .basename("bos");
    let level = std::env::var("BOS_LOG").unwrap_or_else(|_| "error".to_string());
    let level = level.to_lowercase();

    let valid_levels = ["error", "warn", "info", "debug", "trace"];
    let level = if valid_levels.contains(&level.as_str()) {
        level
    } else {
        "error".to_string()
    };


    let logger = Logger::try_with_str(level)
        .unwrap()
        .log_to_file(file_spec)
        .rotate(
            Criterion::Size(10_000_000), // 10 MB
            Naming::TimestampsCustomFormat {
                current_infix: Some(""),
                format: "%Y-%m-%dH%H-%M-%S",
            },
            Cleanup::KeepLogFiles(10),   // 保留 10 个文件
        )
        .duplicate_to_stdout(flexi_logger::Duplicate::All) // 同步输出到控制台
        .format_for_files(flexi_logger::detailed_format)   // 文件日志详细
        // .format_for_stdout(flexi_logger::colored_detailed_format)
        .format_for_stdout(pretty_stdout)
        .start()
        .unwrap();
}

#[cfg(test)]
mod tests {
    use log::{error, info};

    #[test]
    fn it_works() {
        info!("Hello, world!");
        error!("Hello, world!");
    }
}