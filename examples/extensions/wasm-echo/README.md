# Wasm Echo extension

A BSH extension implemented as a WebAssembly Component. The component is built
from `guest/` with `wit-bindgen` and the WIT contract in
`crates/bsh/wit/extension.wit`.

Build the fixture again with:

```text
rustup target add wasm32-unknown-unknown
cd examples/extensions/wasm-echo/guest
cargo build --target wasm32-unknown-unknown --release
wasm-tools component new \
  target/wasm32-unknown-unknown/release/bsh_wasm_echo.wasm \
  -o ../wasm/extension.wasm
```

Install and inspect it:

```text
bsh ext validate examples/extensions/wasm-echo
bsh ext install examples/extensions/wasm-echo
bsh ext list
```

The component exports one tool, `echo`, a command and a panel. It imports only
`bsh:extension/types`; the host links that interface and the `host` functions
when a package uses them.
