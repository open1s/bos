# Hello extension

A minimal declarative BSH extension. It contributes:

- the tool `hello__echo`;
- the command `hello__greet`;
- the prompt fragment in `prompts/hello.md`;
- the panel `hello__status`.

Install and inspect it:

```text
bsh ext validate examples/extensions/hello
bsh ext install examples/extensions/hello
bsh ext list
bsh ext info com.example.hello
bsh ext doctor
```

The package executes no code, so it needs no capability beyond tool
registration and the read-only defaults.
