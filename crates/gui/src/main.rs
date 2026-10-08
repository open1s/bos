//! `bos-gui` binary: launch the BOS chat GUI.

fn main() {
    if let Err(err) = gui::run() {
        eprintln!("bos-gui failed to start: {err:?}");
        std::process::exit(1);
    }
}
