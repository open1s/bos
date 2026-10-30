# Process Echo extension

A BSH process extension. The host starts `python3 server.py` in this directory,
clears the environment except PATH, HOME, TMPDIR, LANG and LC_ALL, and speaks
newline-delimited JSON-RPC 2.0 over stdin/stdout.

`process.execute` is a critical capability, so a local package is denied until
the reader grants it:

```text
bsh ext validate examples/extensions/process-echo
bsh ext grant com.example.process-echo process.execute --scope any --note "local example"
bsh ext install examples/extensions/process-echo
bsh ext info com.example.process-echo
```

The tool is advertised as `process-echo__echo` before the process starts; the
first call spawns the child, performs the initialize handshake and then sends
tools/call.
