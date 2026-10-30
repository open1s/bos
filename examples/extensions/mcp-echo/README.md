# MCP Echo extension

A BSH package whose runtime is a Model Context Protocol server. The adapter
starts the server, performs initialize and notifications/initialized, and maps:

- tools/list to BSH tool descriptors;
- resources/list + resources/read to context contributions;
- prompts/list + prompts/get to prompt contributions.

Install with the process grant:

~~~text
bsh ext grant com.example.mcp-echo process.execute --scope any
bsh ext install examples/extensions/mcp-echo
bsh ext info com.example.mcp-echo
~~~
