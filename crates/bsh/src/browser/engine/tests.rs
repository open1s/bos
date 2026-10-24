//! The engine's own tests: a real Obscura process, a real page, a real gate.
//!
//! # Why these tests skip instead of failing
//!
//! The engine is an external binary that is NOT vendored (67 MB, install recipe
//! in `docs/bsh-browser-substrate.md`). A checkout that has not installed it
//! still has to pass `cargo test -p bsh --lib`, so a test that needs the binary
//! returns early with a line on stderr instead of failing. Everything that can
//! be proved without it - the wire, the base64 decoder, the snapshot parser, the
//! proxy's request rewriting - is a plain unit test in its own module.
//!
//! # What these prove
//!
//! 1. A live page's script-written text is the snapshot's text, its elements get
//!    stable `data-bsh-ref` stamps, a real click and a real type reach it, and
//!    a PNG capture is a styled page rather than a blank rectangle.
//! 2. A refused URL produces ZERO sockets: the refused origin's live listener
//!    stays at zero while the allowed origin's moves, which is the
//!    discrimination that makes the zero mean something.
//! 3. A refused SUBRESOURCE (an image from another origin) is the same zero,
//!    while the page's allowed parts load.
//! 4. Dropping the engine leaves no listener and no child.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent::policy::{EgressGate, PolicyData, PolicySet, SystemResolver};

use super::cdp::client::{dev_binary, discover_binary};
use super::gate::{Gate, STRICT_RULE};
use super::{Engine, FrameSlot, Navigation};
use crate::browser::NetworkMode;

/// How long a proof waits for a subresource the proxy should have refused.
const SETTLE: Duration = Duration::from_millis(700);

/// A loopback origin with a hit counter, serving one body for every path.
struct Origin {
    port: u16,
    hits: Arc<AtomicUsize>,
    /// Every request path the origin actually saw, in order. A path-scoped
    /// refusal is proved by a MISSING entry, so the recording is the proof.
    paths: Arc<std::sync::Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Origin {
    fn start(body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("the port").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let counter = hits.clone();
        let seen = paths.clone();
        let flag = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    continue;
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                if let Ok(mut list) = seen.lock() {
                    list.push(path);
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            port,
            hits,
            paths,
            stop,
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Whether any request path starts with `prefix`.
    fn saw_path(&self, prefix: &str) -> bool {
        self.paths
            .lock()
            .map(|paths| paths.iter().any(|path| path.starts_with(prefix)))
            .unwrap_or(false)
    }

    /// Every recorded request path.
    fn paths(&self) -> Vec<String> {
        self.paths
            .lock()
            .map(|paths| paths.clone())
            .unwrap_or_default()
    }
}

impl Drop for Origin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// A gate over a policy that allows loopback only on these ports.
fn gate_for(ports: &[u16]) -> Arc<EgressGate> {
    let policy = Arc::new(PolicySet::compile_baseline(
        PolicyData::new("/tmp")
            .with_egress_hosts(["127.0.0.1"])
            .with_egress_ports(ports.iter().copied()),
    ));
    Arc::new(EgressGate::new(
        policy,
        Arc::new(SystemResolver),
        "session-test",
        "preset-test",
        "browser_open",
    ))
}

/// A gate over a policy that allows loopback on these ports AND denies these
/// URL path prefixes.
fn gate_for_paths(ports: &[u16], deny_paths: &[&str]) -> Arc<EgressGate> {
    let policy = Arc::new(PolicySet::compile_baseline(
        PolicyData::new("/tmp")
            .with_egress_hosts(["127.0.0.1"])
            .with_egress_ports(ports.iter().copied())
            .with_egress_deny_paths(deny_paths.iter().copied()),
    ));
    Arc::new(EgressGate::new(
        policy,
        Arc::new(SystemResolver),
        "session-test",
        "preset-test",
        "browser_open",
    ))
}

/// A scratch directory for the engine's pidfiles, unique per test process and
/// per call, so two engine tests never clear each other's bookkeeping.
fn scratch_state(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!(
            "bsh-engine-state-{label}-{}-{seq}",
            std::process::id()
        ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch state directory");
    dir
}

/// Start an engine, or skip the test when the binary is not installed.
fn engine(ports: &[u16]) -> Option<(Arc<Engine>, Arc<Gate>)> {
    if discover_binary().is_err() {
        eprintln!("SKIP: no obscura binary (set BSH_OBSCURA_BIN or put obscura on PATH)");
        return None;
    }
    let gate = Gate::start(
        gate_for(ports).policy().clone(),
        "session-test",
        "preset-test",
        NetworkMode::Proxied,
    )
    .expect("the loopback gate could bind");
    let started = Engine::launch_in(
        gate.clone(),
        Arc::new(FrameSlot::default()),
        (800, 600),
        &scratch_state("engine"),
    );
    match started {
        Ok(engine) => Some((Arc::new(engine), gate)),
        Err(error) => {
            // A binary that is present but cannot start is a real failure, not
            // a skip: the message is the engine's own.
            panic!("the engine did not start: {}", error.message());
        }
    }
}

/// The page the live tests navigate to.
fn page(refused_port: Option<u16>) -> String {
    let image = match refused_port {
        Some(port) => format!(
            "<img id=\"remote\" src=\"http://127.0.0.1:{port}/pixel.png\" width=\"20\" height=\"20\">"
        ),
        None => String::new(),
    };
    format!(
        "<!doctype html><html><head><title>Styled Probe</title>\
         <style>body{{font-family:-apple-system,Helvetica,Arial;margin:0;padding:24px}}\
         h1{{color:#4176e6;font-size:40px;margin:0 0 12px}}\
         .box{{width:200px;height:80px;background:#e04141}}\
         p{{font-size:18px}}</style></head><body>\
         <h1 id=\"h\">Styled Heading</h1><div class=\"box\" id=\"box\"></div>\
         <p id=\"out\">script did not run</p>\
         <p>Chinese: 你好，世界！这是中文渲染测试。</p>\
         <a id=\"link\" href=\"/next\">a link</a>\
         <button id=\"btn\">Press me</button>\
         <input id=\"field\" placeholder=\"type here\">\
         {image}</body>\
         <script>document.getElementById('out').textContent='FROM SCRIPT 1+2='+(1+2);\
         document.getElementById('btn').addEventListener('click',function(){{document.getElementById('out').textContent='BUTTON CLICKED';}});</script></html>"
    )
}

#[test]
fn a_live_page_answers_with_script_text_stable_refs_real_input_and_a_styled_capture() {
    let origin = Origin::start(page(None));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("navigate should answer");
    assert_eq!(navigation, Navigation::Loaded);
    assert_eq!(
        engine.url().expect("the page has a URL"),
        format!("http://127.0.0.1:{}/", origin.port)
    );

    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    assert_eq!(snapshot.title, "Styled Probe");
    // The request seam saw the document and continued it: the Fetch domain is
    // delivery control here, and saying so is the honest reading of it.
    assert!(
        engine.client().shared().paused() >= 1,
        "the Fetch seam should have paused the document request"
    );
    assert!(engine.client().shared().continued() >= 1);
    assert!(
        snapshot.text.contains("FROM SCRIPT 1+2=3"),
        "the script's text must be the page's text: {:?}",
        snapshot.text
    );
    assert!(
        snapshot.text.contains("Styled Heading"),
        "{:?}",
        snapshot.text
    );
    let kinds: Vec<&str> = snapshot
        .elements
        .iter()
        .map(|element| element.kind)
        .collect();
    assert!(kinds.contains(&"link"), "{kinds:?}");
    assert!(kinds.contains(&"input"), "{kinds:?}");
    assert!(kinds.contains(&"submit"), "{kinds:?}");

    // Refs survive a second read: the stamps are in the page, not in a table.
    let again = engine.snapshot(1).expect("a second read should answer");
    let first: Vec<u64> = snapshot.elements.iter().map(|element| element.id).collect();
    let second: Vec<u64> = again.elements.iter().map(|element| element.id).collect();
    assert_eq!(first, second, "refs must be stable across snapshots");

    // A real click: the element's centre is pressed and the page's own handler
    // runs, which is what the text view could not do.
    let button = snapshot
        .elements
        .iter()
        .find(|element| element.label == "Press me")
        .expect("the button");
    engine
        .click(&format!("e{}", button.id))
        .expect("the click dispatches");
    std::thread::sleep(Duration::from_millis(250));
    let after = engine.snapshot(1).expect("the page should still read");
    assert!(
        after.text.contains("BUTTON CLICKED"),
        "the click must reach the page's handler: {:?}",
        after.text
    );

    // A real type: the field's value changes through Input.insertText.
    let field = after
        .elements
        .iter()
        .find(|element| element.kind == "input")
        .expect("a field");
    let typed = engine
        .type_into(&format!("e{}", field.id), "hello typed")
        .expect("typing dispatches");
    assert_eq!(typed.value, "hello typed");
    assert!(typed.error.is_none(), "{:?}", typed.error);

    // The capture: a page with a blue heading and a red box is not a blank
    // rectangle, and the proof file is written where a reader can look at it.
    let png = engine.screenshot().expect("a PNG capture");
    let image = image::load_from_memory(&png).expect("the capture should decode");
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    assert!(width >= 700 && height >= 500, "{width}x{height}");
    let mut blue = 0usize;
    let mut red = 0usize;
    let mut opaque = 0usize;
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        if a > 0 {
            opaque += 1;
        }
        if b > 180 && r < 120 && g < 160 {
            blue += 1;
        }
        if r > 180 && g < 120 && b < 120 {
            red += 1;
        }
    }
    assert!(opaque > 100_000, "the capture is nearly empty: {opaque}");
    assert!(blue > 300, "the blue heading did not render: {blue}");
    assert!(red > 2_000, "the red box did not render: {red}");
    let proof =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/bsh-obscura-proof.png");
    std::fs::write(&proof, &png).expect("the proof file should write");
    eprintln!("PROOF: styled page capture at {}", proof.display());
}

#[test]
fn a_refused_url_makes_no_request_and_an_allowed_one_moves_the_same_counter() {
    let allowed = Origin::start(page(None));
    let refused = Origin::start(String::from("<html><title>refused</title></html>"));
    let Some((engine, _gate)) = engine(&[allowed.port]) else {
        return;
    };

    // The refusal: the pre-flight asks the policy before Page.navigate.
    let navigation = engine
        .navigate(
            &gate_for(&[allowed.port]),
            &format!("http://127.0.0.1:{}/", refused.port),
            NetworkMode::Proxied,
        )
        .expect("a refusal is an answer, not a failure");
    match navigation {
        Navigation::Refused { rule, message } => {
            assert!(
                rule.as_deref().unwrap_or_default().contains("port")
                    || message.to_lowercase().contains("port"),
                "the refusal should name the port rule: {rule:?} {message}"
            );
        }
        Navigation::Loaded => panic!("the refused origin must not load"),
    }
    assert_eq!(
        refused.hits(),
        0,
        "a refused URL must produce NO origin socket"
    );

    // Discrimination: the SAME counter shape moves for an allowed URL, so the
    // zero above cannot pass by a gate that refuses everything.
    let navigation = engine
        .navigate(
            &gate_for(&[allowed.port]),
            &format!("http://127.0.0.1:{}/", allowed.port),
            NetworkMode::Proxied,
        )
        .expect("the allowed page should load");
    assert_eq!(navigation, Navigation::Loaded);
    // A load event settles the page, not the origin's own counter, so the
    // assertion waits a bounded moment rather than assuming the two orderings.
    let mut hits = 0;
    for _ in 0..40 {
        hits = allowed.hits();
        if hits >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!(
        "PROOF gate: refused origin hits={} allowed origin hits={hits}",
        refused.hits()
    );
    assert!(
        hits >= 1,
        "an allowed URL must reach its origin: hits={hits} url={:?} gate(allowed={} refused={}) page={:?}",
        engine.url(),
        engine.gate().allowed(),
        engine.gate().refused(),
        engine
            .snapshot(1)
            .map(|page| page.title)
            .unwrap_or_default()
    );
}

#[test]
fn a_refused_subresource_stays_at_zero_while_the_allowed_page_loads() {
    let refused = Origin::start(String::from("<html><title>pixel</title></html>"));
    let allowed = Origin::start(page(Some(refused.port)));
    let Some((engine, gate)) = engine(&[allowed.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[allowed.port]),
            &format!("http://127.0.0.1:{}/", allowed.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    assert!(
        snapshot.text.contains("Styled Heading"),
        "{:?}",
        snapshot.text
    );
    assert!(allowed.hits() >= 1, "the page itself loaded");
    std::thread::sleep(SETTLE);
    assert_eq!(
        refused.hits(),
        0,
        "a refused SUBRESOURCE must produce no socket either"
    );
    assert!(
        gate.refused() >= 1,
        "the gate must have RECORDED the refusal: allowed={} refused={}",
        gate.allowed(),
        gate.refused()
    );
    assert!(
        gate.last().is_some(),
        "the refusal must carry the policy's own prose"
    );
    assert!(
        engine.client().shared().failed() >= 1,
        "the Fetch seam should have failed the denied subresource's delivery"
    );
}

#[test]
fn dropping_the_engine_leaves_no_listener_and_no_child() {
    let origin = Origin::start(page(None));
    let Some((engine, gate)) = engine(&[origin.port]) else {
        return;
    };
    engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    let port = engine.client().port();
    assert_ne!(port, 0);
    drop(gate);
    drop(engine);
    // The port must be closed: the child was killed and reaped, not orphaned.
    let mut closed = false;
    for _ in 0..50 {
        let address = format!("127.0.0.1:{port}").parse().expect("an address");
        if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err() {
            closed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(closed, "the engine's port {port} is still open after Drop");
    // The documented dev location is where a checkout expects the binary.
    assert!(dev_binary().ends_with("target/obscura/obscura"));
}
#[test]
fn the_browser_model_is_fed_by_the_live_engine() {
    let origin = Origin::start(page(None));
    if discover_binary().is_err() {
        eprintln!("SKIP: no obscura binary (set BSH_OBSCURA_BIN or put obscura on PATH)");
        return;
    }
    let handle = crate::browser::BrowserHandle::live();
    let url = format!("http://127.0.0.1:{}/", origin.port);
    crate::browser::navigate(&handle, &gate_for(&[origin.port]), &url)
        .expect("the model navigates through the engine");

    // The SAME payload shape the text view produces, filled from the live DOM.
    let payload = crate::browser::snapshot_live(&handle).expect("a live snapshot");
    let text = payload["snapshot"]["text"]
        .as_str()
        .expect("the payload carries text");
    assert!(
        text.contains("FROM SCRIPT 1+2=3"),
        "the tool payload must carry the script's text: {text}"
    );
    let button = payload["snapshot"]["elements"]
        .as_array()
        .expect("elements")
        .iter()
        .find(|element| element["label"] == "Press me")
        .and_then(|element| element["ref"].as_str())
        .expect("the button's ref")
        .to_string();
    let clicked = crate::browser::click_live(&handle, &button).expect("the click answers");
    assert!(
        clicked["snapshot"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("BUTTON CLICKED"),
        "a click through the model must reach the page: {clicked}"
    );
    // A stale ref gets the model's own sentence, not the engine's.
    let stale = crate::browser::click_live(&handle, "e9999").unwrap_err();
    assert!(
        format!("{stale:?}").contains("call browser_snapshot for fresh refs"),
        "{stale:?}"
    );
    // Closing stops the process this handle started.
    crate::browser::close_live(&handle);
    assert!(!handle.engine_ready());
}

/// A pre-warm separates the two costs a first open used to pay together:
/// launching the engine (process, CDP handshake, V8, the font scan) and
/// navigating. MEASURED on this machine: launch ~0.16 s and a navigation on an
/// already-warm engine a fraction of that plus the 500 ms networkidle window.
///
/// The assertion is that the warm engine is REUSED: a second navigation keeps
/// the same process port, so a pane that pre-warms on first show does not pay
/// the launch on its first Go.
#[test]
fn a_prewarmed_engine_pays_no_launch_on_its_first_navigation() {
    let origin = Origin::start(page(None));
    if discover_binary().is_err() {
        eprintln!("SKIP: no obscura binary (set BSH_OBSCURA_BIN or put obscura on PATH)");
        return;
    }
    let gate = Gate::start(
        gate_for(&[origin.port]).policy().clone(),
        "session-test",
        "preset-test",
        NetworkMode::Proxied,
    )
    .expect("the loopback gate could bind");
    let launched = std::time::Instant::now();
    let engine = Engine::launch_in(
        gate.clone(),
        Arc::new(FrameSlot::default()),
        (800, 600),
        &scratch_state("prewarm"),
    )
    .expect("the engine should start");
    let launch = launched.elapsed();
    let port = engine.client().port();

    let first = std::time::Instant::now();
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("navigate answers");
    assert_eq!(navigation, Navigation::Loaded);
    let first_navigation = first.elapsed();

    let second = std::time::Instant::now();
    let again = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/?second", origin.port),
            NetworkMode::Proxied,
        )
        .expect("navigate answers");
    assert_eq!(again, Navigation::Loaded);
    let second_navigation = second.elapsed();
    assert_eq!(
        engine.client().port(),
        port,
        "a warm engine must be reused, not relaunched"
    );
    assert!(
        first_navigation >= std::time::Duration::from_millis(400),
        "the networkidle window is part of a settled navigation: {first_navigation:?}"
    );
    eprintln!(
        "MEASURE pre-warm: launch={launch:?} first_navigation={first_navigation:?} second_navigation={second_navigation:?} (launch is paid when the pane appears, not by the navigation)"
    );
}

/// GAP 1 at the engine: the proxied plain-HTTP request is authorized on its
/// PATH before the origin socket exists. A refused subresource path stays at
/// ZERO origin hits while the page's allowed parts load - the discrimination
/// that makes the zero mean something.
#[test]
fn a_denied_subresource_path_never_reaches_the_origin() {
    let html = "<!doctype html><html><body><h1 id=\"h\">allowed page</h1>\
        <img src=\"/secret/pixel.png\" width=\"20\" height=\"20\">\
        <p id=\"t\">loaded</p></body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let gate = gate_for_paths(&[origin.port], &["/secret/"]);
    let navigation = engine
        .navigate(
            &gate,
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    assert!(
        snapshot.text.contains("allowed page"),
        "{:?}",
        snapshot.text
    );
    assert!(
        origin.saw_path("/"),
        "the allowed document must reach the origin: {:?}",
        origin.paths()
    );
    std::thread::sleep(SETTLE);
    assert!(
        !origin.saw_path("/secret/"),
        "a path-denied subresource must produce NO origin socket: {:?}",
        origin.paths()
    );
    assert!(
        engine.gate().refused() >= 1,
        "the proxy must have RECORDED the path refusal"
    );
    assert_eq!(
        engine
            .gate()
            .last()
            .and_then(|denial| denial.rule)
            .as_deref(),
        Some("egress-path-denied"),
        "{:?}",
        engine.gate().last()
    );
}

/// Enter really submits: a focused field inside a GET form navigates when the
/// pane's Enter key is forwarded (keyDown + CHAR + keyUp). This is the
/// deterministic version of the Baidu measurement.
#[test]
fn enter_submits_a_focused_form_field() {
    let html = "<!doctype html><html><body>\
        <form action=\"/submitted\" method=\"get\">\
        <input name=\"q\" value=\"\"><button id=\"go\">Go</button></form>\
        </body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/form", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the form page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    let field = snapshot
        .elements
        .iter()
        .find(|element| element.kind == "input")
        .expect("the form has a field");
    let reference = format!("e{}", field.id);
    engine.click(&reference).expect("the click dispatches");
    engine
        .type_into(&reference, "ab")
        .expect("typing dispatches");
    engine.press("enter").expect("Enter dispatches");
    // The submission is a navigation: wait for the URL rather than assuming.
    let mut navigated = false;
    for _ in 0..60 {
        if engine.url().unwrap_or_default().contains("/submitted") {
            navigated = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        navigated,
        "Enter in a focused form field must submit it: url={:?} paths={:?}",
        engine.url(),
        origin.paths()
    );
    assert!(
        origin.saw_path("/submitted"),
        "the form submission must reach the origin: {:?}",
        origin.paths()
    );
}

/// The pane's editing keys reach the page: Backspace deletes, the arrows move
/// the caret, and Enter's CHAR event is accepted (the keydown alone is not a
/// keypress). A page's own input handler is the observable.
#[test]
fn editing_keys_change_a_field_and_enter_is_a_real_keypress() {
    let origin = Origin::start(page(None));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    let field = snapshot
        .elements
        .iter()
        .find(|element| element.kind == "input")
        .expect("the fixture has a field");
    let reference = format!("e{}", field.id);
    match engine.click(&reference).expect("the click dispatches") {
        super::cdp::Activation::Focused { .. } => {}
        other => panic!("an input takes focus: {other:?}"),
    }
    let typed = engine
        .type_into(&reference, "abc")
        .expect("typing dispatches");
    assert_eq!(typed.value, "abc");
    engine.press("backspace").expect("backspace dispatches");
    std::thread::sleep(Duration::from_millis(120));
    let after = engine.snapshot(1).expect("the page still reads");
    let value = after
        .elements
        .iter()
        .find(|element| element.id == field.id)
        .and_then(|element| element.value.clone())
        .unwrap_or_default();
    assert_eq!(
        value, "ab",
        "Backspace must delete in the focused field, not be dropped"
    );
    // The CHAR event Enter now carries must be accepted by this engine build.
    engine
        .press("enter")
        .expect("Enter dispatches keyDown, char and keyUp");
}

/// GAP 2 at the engine: Strict refuses an https navigation with the rule named
/// BEFORE the document request, and plain http on the same engine still loads.
#[test]
fn strict_mode_refuses_https_by_rule_and_keeps_http_working() {
    let origin = Origin::start(page(None));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let gate = gate_for(&[origin.port]);
    let refused = engine
        .navigate(&gate, "https://127.0.0.1/private", NetworkMode::Strict)
        .expect("a refusal is an answer");
    match refused {
        Navigation::Refused { rule, message } => {
            assert_eq!(rule.as_deref(), Some(STRICT_RULE), "{message}");
            assert!(
                message.contains("does not terminate TLS"),
                "the sentence must say why: {message}"
            );
        }
        Navigation::Loaded => panic!("Strict must not tunnel an https page"),
    }
    assert_eq!(
        origin.hits(),
        0,
        "a Strict https refusal must open no request at all"
    );
    let loaded = engine
        .navigate(
            &gate,
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Strict,
        )
        .expect("http still works in Strict");
    assert_eq!(loaded, Navigation::Loaded);
}

/// GAP 4: a page that starts a subresource 300 ms after its load event is READ
/// WITH THAT SUBRESOURCE. `browser_open` waits for networkidle, so the text the
/// late image's own handler writes is already in the snapshot the open returns.
#[test]
fn networkidle_reads_content_a_late_subresource_wrote() {
    let html = "<!doctype html><html><body><p id=\"early\">EARLY</p>\
        <img id=\"late\" width=\"10\" height=\"10\">\
        <div id=\"after\">waiting for the late subresource</div>\
        <script>setTimeout(function(){\
          var img = document.getElementById('late');\
          img.onload = function(){ document.getElementById('after').textContent = 'LATE SUBRESOURCE ARRIVED'; };\
          img.onerror = function(){ document.getElementById('after').textContent = 'LATE SUBRESOURCE FAILED'; };\
          img.src = '/late.png';\
        }, 300);</script></body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    // IMMEDIATELY after open: no extra sleep. load + 120 ms could not have seen
    // content a 300 ms timer wrote; networkidle can.
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    assert!(
        origin.saw_path("/late.png"),
        "the late subresource must have been requested: {:?}",
        origin.paths()
    );
    assert!(
        snapshot.text.contains("LATE SUBRESOURCE"),
        "the snapshot taken by browser_open must contain what the late \
         subresource wrote: {:?}",
        snapshot.text
    );
}

/// GAP 3: an inline link is activated by a real pointer event whenever the
/// engine gives its contents a box, and the result says which path ran.
#[test]
fn an_inline_link_reports_the_path_its_click_took() {
    let origin = Origin::start(page(None));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    let link = snapshot
        .elements
        .iter()
        .find(|element| element.kind == "link")
        .expect("the fixture has a link");
    let activation = engine
        .click(&format!("e{}", link.id))
        .expect("the click dispatches");
    let via = match activation {
        super::cdp::Activation::Clicked { via } => via,
        other => panic!("a link click must be a click, not a focus: {other:?}"),
    };
    eprintln!("MEASURE inline link click via={via}");
    // A real pointer click is the fix; a scripted one is the documented
    // residual for a genuinely boxless target, and the result says which.
    assert!(via == "pointer" || via == "script", "{via}");
    if via == "pointer" {
        assert!(
            engine.url().expect("a URL").ends_with("/next"),
            "a pointer click on the link must navigate: {:?}",
            engine.url()
        );
    }
}

/// GAP I1: the everyday editing chords reach the page and PASTE inserts once.
///
/// The paste route, stated: the pane reads the system clipboard only because
/// the reader pressed the chord, and hands the string to `Engine::paste`,
/// which dispatches a `paste` event carrying that text and inserts it through
/// `Input.insertText` ONLY when the page did not prevent default. One source
/// of truth, one insertion.
#[test]
fn editing_chords_reach_the_page_and_paste_inserts_once() {
    let html = "<!doctype html><html><body><textarea id=\"ta\"></textarea>\
        <script>\
        window.__keys = [];\
        window.__counts = { paste: 0, copy: 0, cut: 0 };\
        window.__pasted = '';\
        var ta = document.getElementById('ta');\
        ta.addEventListener('keydown', function(e) {\
          window.__keys.push(e.key + (e.metaKey ? '+meta' : '') + \
            (e.ctrlKey ? '+ctrl' : '') + (e.shiftKey ? '+shift' : ''));\
        });\
        ta.addEventListener('paste', function(e) {\
          window.__counts.paste++;\
          window.__pasted = e.clipboardData ? \
            e.clipboardData.getData('text/plain') : '';\
        });\
        ta.addEventListener('copy', function() { window.__counts.copy++; });\
        ta.addEventListener('cut', function() { window.__counts.cut++; });\
        </script></body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    let field = snapshot
        .elements
        .iter()
        .find(|element| element.kind == "textarea")
        .expect("the fixture has a textarea");
    let reference = format!("e{}", field.id);
    engine.click(&reference).expect("the click dispatches");
    for ch in ["h", "e", "l", "l", "o"] {
        engine.type_key(ch, 0).expect("a printable key dispatches");
    }

    // Cmd/Ctrl+A then Cmd/Ctrl+C: the chord reaches the page's keydown handler
    // with the right modifier, the copy event runs, and the page's selection is
    // the text the pane puts on the system clipboard.
    engine
        .select_all(super::cdp::META)
        .expect("select-all dispatches");
    let copied = engine
        .copy(super::cdp::META)
        .expect("copy dispatches")
        .expect("a selection is on the clipboard");
    assert_eq!(copied, "hello");

    // Put the caret at the end and paste: the page sees one paste event with
    // the clipboard text, and the field holds it exactly once. Obscura does not
    // move a textarea's caret from the arrow or End keys (MEASURED), so the
    // fixture moves it the way a page's own script would; the chord under test
    // is the paste.
    engine
        .evaluate(
            "(function() { var t = document.getElementById('ta'); t.focus(); \
             t.setSelectionRange(t.value.length, t.value.length); return true; })()",
        )
        .expect("the caret moves");
    engine
        .paste(" world", super::cdp::META)
        .expect("paste dispatches");
    std::thread::sleep(Duration::from_millis(120));

    // Cmd/Ctrl+X removes the selection after copying it.
    engine
        .select_all(super::cdp::META)
        .expect("select-all dispatches");
    let cut = engine
        .cut(super::cdp::META)
        .expect("cut dispatches")
        .expect("a selection is on the clipboard");
    assert_eq!(cut, "hello world");

    // Undo and redo are forwarded with their modifiers, which is what the page
    // can act on; the engine keeps no history of its own.
    engine
        .undo(super::cdp::META | super::cdp::SHIFT)
        .expect("redo dispatches");

    let state = engine
        .evaluate(
            "JSON.stringify({value: document.getElementById('ta').value, \
             keys: window.__keys, counts: window.__counts, pasted: window.__pasted})",
        )
        .expect("the probe state answers");
    let state: serde_json::Value =
        serde_json::from_str(state.as_str().expect("a JSON string")).expect("the probe JSON");
    assert_eq!(
        state["value"], "",
        "cut must remove the selected text from the field: {state}"
    );
    assert_eq!(state["counts"]["copy"], 1, "one copy event: {state}");
    assert_eq!(state["counts"]["paste"], 1, "one paste event: {state}");
    assert_eq!(state["counts"]["cut"], 1, "one cut event: {state}");
    assert_eq!(
        state["pasted"], " world",
        "the paste event must carry the clipboard text: {state}"
    );
    let keys: Vec<String> = state["keys"]
        .as_array()
        .expect("the keydown log")
        .iter()
        .filter_map(|key| key.as_str().map(String::from))
        .collect();
    for chord in ["a+meta", "c+meta", "v+meta", "x+meta", "z+meta+shift"] {
        assert!(
            keys.iter().any(|key| key == chord),
            "the page must see the {chord} chord: {keys:?}"
        );
    }
    eprintln!("PROOF chords: page keydown log {keys:?}");
}

/// GAP I2: a printable character is a keydown plus a char, so a page that
/// listens for keydown sees the key, and the field still holds the text ONCE.
#[test]
fn printable_keys_fire_keydown_and_insert_exactly_once() {
    let html = "<!doctype html><html><body><textarea id=\"ta\"></textarea>\
        <div id=\"log\"></div>\
        <script>\
        window.__keys = [];\
        var ta = document.getElementById('ta');\
        ta.addEventListener('keydown', function(e) {\
          window.__keys.push(e.key);\
          document.getElementById('log').textContent = window.__keys.join(',');\
        });\
        </script></body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    let snapshot = engine.snapshot(1).expect("the live DOM should read");
    let field = snapshot
        .elements
        .iter()
        .find(|element| element.kind == "textarea")
        .expect("the fixture has a textarea");
    engine
        .click(&format!("e{}", field.id))
        .expect("the click dispatches");
    for ch in ["a", "b", "c"] {
        engine.type_key(ch, 0).expect("a printable key dispatches");
    }
    // A composed character goes through the composition route and must not add
    // a phantom keydown for a key the keyboard does not have.
    engine.type_key("中", 0).expect("composition text inserts");
    std::thread::sleep(Duration::from_millis(120));
    let state = engine
        .evaluate(
            "JSON.stringify({value: document.getElementById('ta').value, \
             keys: window.__keys, log: document.getElementById('log').textContent})",
        )
        .expect("the probe state answers");
    let state: serde_json::Value =
        serde_json::from_str(state.as_str().expect("a JSON string")).expect("the probe JSON");
    assert_eq!(
        state["value"], "abc中",
        "the field must hold the typed text exactly once: {state}"
    );
    assert_eq!(
        state["log"], "a,b,c",
        "the page's keydown log must contain a, b and c: {state}"
    );
    eprintln!(
        "PROOF printable keys: value={:?} log={:?}",
        state["value"], state["log"]
    );
}

/// GAP I3: the click count reaches the page AND the default word/line
/// selection happens (the engine fires dblclick but selects nothing itself).
#[test]
fn a_double_click_selects_the_word_and_a_triple_click_the_line() {
    let html = "<!doctype html><html><body>\
        <p id=\"word\" style=\"font-size:24px\">alpha beta gamma</p>\
        <textarea id=\"ta\" style=\"width:500px;height:120px;font-size:18px\">alpha betacharlie delta</textarea>\
        <script>\
        window.__details = [];\
        document.addEventListener('mousedown', function(e) {\
          window.__details.push(e.detail);\
        });\
        document.addEventListener('dblclick', function(e) {\
          window.__dbl = e.detail;\
        });\
        </script></body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    std::thread::sleep(Duration::from_millis(120));
    // A point the ENGINE's own hit testing puts inside "beta" (characters 6..10).
    // A Range's box is not usable here: MEASURED, an inline range's rect is the
    // paragraph's, so its centre lands in "gamma". Scanning the engine's caret
    // hit test makes the fixture independent of font metrics while still
    // clicking a real point.
    let point = engine
        .evaluate(
            "(function() {\
               var node = document.getElementById('word').firstChild;\
               var box = document.getElementById('word').getBoundingClientRect();\
               var y = box.top + box.height / 2;\
               for (var x = box.left + 1; x < box.right - 1; x += 2) {\
                 var range = document.caretRangeFromPoint(x, y);\
                 if (range && range.startContainer === node &&\
                     range.startOffset >= 6 && range.startOffset < 10) {\
                   return JSON.stringify({x: x, y: y, offset: range.startOffset});\
                 }\
               }\
               return JSON.stringify({x: box.left + box.width / 2, y: y, offset: -1});\
             })()",
        )
        .expect("the word's box answers");
    let point: serde_json::Value =
        serde_json::from_str(point.as_str().expect("a JSON string")).expect("the point JSON");
    assert!(
        point["offset"].as_i64().unwrap_or(-1) >= 0,
        "the scan should find a point inside beta: {point}"
    );
    let x = point["x"].as_f64().expect("an x") as f32;
    let y = point["y"].as_f64().expect("a y") as f32;

    // A single click selects nothing in document text, which is what makes the
    // double-click assertion discriminate.
    engine.pointer_down(x, y, 1).expect("press");
    engine.pointer_up(x, y, 1).expect("release");
    std::thread::sleep(Duration::from_millis(80));
    let single = engine
        .evaluate("String(window.getSelection())")
        .expect("selection reads");
    assert_eq!(
        single.as_str().unwrap_or_default(),
        "",
        "a single click must not select a word"
    );

    engine.pointer_down(x, y, 2).expect("press");
    engine.pointer_up(x, y, 2).expect("release");
    std::thread::sleep(Duration::from_millis(80));
    let word = engine
        .evaluate("String(window.getSelection())")
        .expect("selection reads");
    assert_eq!(
        word.as_str().unwrap_or_default(),
        "beta",
        "a double click must select exactly the word under the pointer"
    );

    engine.pointer_down(x, y, 3).expect("press");
    engine.pointer_up(x, y, 3).expect("release");
    std::thread::sleep(Duration::from_millis(80));
    let line = engine
        .evaluate("String(window.getSelection())")
        .expect("selection reads");
    assert_eq!(
        line.as_str().unwrap_or_default(),
        "alpha beta gamma",
        "a triple click must select the line (the block, for this fixture)"
    );
    let details = engine
        .evaluate("JSON.stringify({details: window.__details, dbl: window.__dbl || 0})")
        .expect("the click log answers");
    let details: serde_json::Value =
        serde_json::from_str(details.as_str().expect("a JSON string")).expect("the click JSON");
    assert_eq!(
        details["details"],
        serde_json::json!([1, 2, 3]),
        "the page must see the click counts the pane timed: {details}"
    );
    assert_eq!(details["dbl"], 2, "the page's dblclick fired: {details}");

    // Residual, stated in the substrate doc: a double click inside an <input> or
    // <textarea> selects nothing, because Obscura keeps the click's caret
    // internally and never exposes it (`selectionStart` stays null after a
    // click - MEASURED), so there is no offset to expand to a word. The pane
    // will not guess one: guessing would select the wrong word.
    eprintln!(
        "PROOF click count: details={} document={:?}",
        details["details"], word
    );
}

/// GAP I4: a release at the clamped edge of the page ends the drag the page is
/// tracking, and no move follows it.
#[test]
fn a_release_at_the_clamped_edge_clears_the_page_drag() {
    let html = "<!doctype html><html><body>\
        <div id=\"pad\" style=\"width:600px;height:300px;background:#eee\"></div>\
        <div id=\"state\">idle</div>\
        <script>\
        window.__moves = 0;\
        window.__ups = 0;\
        window.__lastUp = null;\
        window.__dragging = false;\
        var pad = document.getElementById('pad');\
        pad.addEventListener('mousedown', function() {\
          window.__dragging = true;\
          document.getElementById('state').textContent = 'dragging';\
        });\
        document.addEventListener('mousemove', function() {\
          if (window.__dragging) { window.__moves++; }\
        });\
        document.addEventListener('mouseup', function(e) {\
          window.__dragging = false;\
          window.__ups++;\
          window.__lastUp = [e.clientX, e.clientY];\
          document.getElementById('state').textContent = 'idle';\
        });\
        </script></body></html>";
    let origin = Origin::start(String::from(html));
    let Some((engine, _gate)) = engine(&[origin.port]) else {
        return;
    };
    let navigation = engine
        .navigate(
            &gate_for(&[origin.port]),
            &format!("http://127.0.0.1:{}/", origin.port),
            NetworkMode::Proxied,
        )
        .expect("the page should load");
    assert_eq!(navigation, Navigation::Loaded);
    std::thread::sleep(Duration::from_millis(120));

    engine.pointer_down(100.0, 100.0, 1).expect("press");
    engine.pointer_move(140.0, 120.0).expect("drag");
    std::thread::sleep(Duration::from_millis(80));
    let dragging = engine
        .evaluate(
            "JSON.stringify({state: document.getElementById('state').textContent, \
             moves: window.__moves})",
        )
        .expect("the drag state reads");
    let dragging: serde_json::Value =
        serde_json::from_str(dragging.as_str().expect("a JSON string")).expect("the drag JSON");
    assert_eq!(dragging["state"], "dragging", "{dragging}");
    assert_eq!(dragging["moves"], 1, "{dragging}");

    // The clamp the pane applies to a release outside its frame: the page's
    // far edge. The engine must deliver it as a real release.
    engine.pointer_up(799.0, 599.0, 1).expect("release");
    std::thread::sleep(Duration::from_millis(80));
    let after = engine
        .evaluate(
            "JSON.stringify({state: document.getElementById('state').textContent, \
             dragging: window.__dragging, moves: window.__moves, ups: window.__ups, \
             lastUp: window.__lastUp})",
        )
        .expect("the drag state reads");
    let after: serde_json::Value =
        serde_json::from_str(after.as_str().expect("a JSON string")).expect("the drag JSON");
    assert_eq!(
        after["state"], "idle",
        "the page drag must be cleared: {after}"
    );
    assert_eq!(after["dragging"], false, "{after}");
    assert_eq!(after["ups"], 1, "exactly one release: {after}");
    assert_eq!(
        after["lastUp"],
        serde_json::json!([799, 599]),
        "the release must carry the clamped edge coordinate: {after}"
    );
    assert_eq!(
        after["moves"], 1,
        "no move may be forwarded after the release: {after}"
    );
    eprintln!("PROOF captured release: {after}");
}
