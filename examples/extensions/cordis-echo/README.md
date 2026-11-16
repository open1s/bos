# Cordis Echo

A real DeepSeek Harness / Cordis package. Its whole payload is apply(ctx,
config); BSH runs it through the sandboxed Node bridge and maps its tool
registration to a BSH tool.

Install:

~~~text
bsh ext grant com.example.cordis-echo process.execute
bsh ext install examples/extensions/cordis-echo
bsh ext info com.example.cordis-echo
~~~
