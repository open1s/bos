---
name: allinone
description: Walk the reader through the BSH extension framework using the all-in-one demo
version: 1.0.0
---

# All-in-One Demo

Use this skill when the reader wants a guided tour of the BSH extension
framework.

1. Call `describe` to show the package identity, runtime, configuration and
   contributing surfaces.
2. Call `events` to show which host topics have reached the process.
3. Call `config` to show the configuration the host passed in the initialize
   handshake.
4. Explain that `inspect` is a workflow wrapping the same tools, and that the
   panel buttons dispatch `commands/execute` back into this process.
