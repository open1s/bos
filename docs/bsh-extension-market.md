---
module: bsh
tags: [extensions, market, git]
problem_type: reference
---

# BSH Extension Market (Git provider)

The market is a Git repository, not a bespoke registry service. The official
provider is `https://github.com/open1s/bsh_extensions`; a private provider uses
the same contract.

## Repository layout

~~~text
bsh_extensions/
  index.json
  packages/
    <id>/
      <version>/            one package root (extension.toml, tools/, ...)
~~~

`index.json` is the only document the host reads:

~~~json
{
  "schema_version": 1,
  "extensions": [
    {
      "id": "com.example.hello",
      "name": "Hello",
      "version": "1.0.0",
      "description": "A greeting package",
      "path": "packages/com.example.hello/1.0.0",
      "hash": "sha256-optional"
    }
  ]
}
~~~

Rules:

- `schema_version` is exactly 1.
- `id` follows the reverse-domain extension grammar.
- `version` is SemVer.
- `path` is relative, must not contain `..`, and must point at a directory
  whose root holds `extension.toml`.
- `hash` is optional; when present the store's own package hash is still
  computed and recorded.

## Host behaviour

`bsh ext market [query]`:

1. clones the provider shallowly into `<extension root>/market/<repo>`, or
   fetches and hard-resets an existing checkout;
2. reads and validates `index.json`;
3. prints `ID / VERSION / NAME / PATH` for matching entries.

`bsh ext market install <id>` installs the newest SemVer entry with community
trust and records the Git repository plus revision as the package source. All
normal checks still run: manifest validation, package hash, signature policy,
capability grants, lifecycle and audit.

The provider URL can be overridden for development or a private registry:

~~~text
BSH_EXTENSION_MARKET=/path/to/market bsh ext market
BSH_EXTENSION_MARKET=https://github.com/acme/bsh_extensions bsh ext market
~~~

## Why Git

- the index and packages are reviewable in the same pull request;
- no separate server, database or JS runtime;
- a private provider is just a private repository;
- the installed package is still copied into the hashed store, so the market is
  a distribution channel and never the runtime.
