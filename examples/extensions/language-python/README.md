# Python Language + GitHub Integration

A starter for the two extension types that usually need code: a language
service and an external-system integration.

## What it demonstrates

- `languages/python.json` declares the bare `py` extension and the
  `diagnose` tool.
- `integrations/github.json` declares a GitHub integration and the
  `github-search` tool.
- Both actions are ordinary tools of the same process package, so the agent can
  call them and the host can dispatch them through the CLI.
- A real `ast.parse` diagnostic, not a stub.

## Try it

~~~text
bsh ext install examples/extensions/language-python
bsh ext grant com.example.language-python process.execute --scope any
bsh ext grant com.example.language-python agent.tools.register --scope any

printf 'def ok():\n    return 1\n' > /tmp/ok.py
bsh ext language-check com.example.language-python python /tmp/ok.py

bsh ext integrate com.example.language-python github '{"query":"bsh"}'
~~~

## Adapt it

1. Replace `python.json` with your language id and extensions.
2. Keep `tool` pointing at a tool that accepts `{path,text,language}` and
   returns `{ok,diagnostics}`.
3. For an integration, declare `kind` as github, browser, terminal or custom
   and point `action` at a tool or command.
4. Keep the process handshake: echo the token from `initialize`.
