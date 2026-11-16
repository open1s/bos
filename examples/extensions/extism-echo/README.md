# Extism Echo

A BSH package whose runtime is an Extism plugin. The guest exports:

- bsh_tools, bsh_call_tool, bsh_command, bsh_panel, bsh_event;
- one `bsh_host` host function for policy-checked file, network, storage,
  secret, config, event and clock operations.

The manifest also declares prompts, contexts, commands, a panel, a theme and
model/agent provider descriptors, so every host surface is enhanced by the same
package. Build the guest with:

~~~text
rustup target add wasm32-unknown-unknown
cd guest
cargo build --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/bsh_extism_echo.wasm ../plugin.wasm
~~~

Install with:

~~~text
bsh ext install examples/extensions/extism-echo
bsh ext grant com.example.extism-echo agent.tools.register --scope any
bsh ext info com.example.extism-echo
~~~
