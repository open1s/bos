//! `bsh` binary: launch the BOS chat GUI.

fn main() {
    if let Err(err) = bshlib::run() {
        eprintln!("bsh failed to start: {err:?}");
        std::process::exit(1);
    }
}
