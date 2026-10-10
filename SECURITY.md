# Security Policy

## Reporting a Vulnerability

Please report suspected security vulnerabilities privately. Do **not** open a
public issue, pull request, or discussion for a security problem.

- **Email:** [oss@open1s.com](mailto:oss@open1s.com)
- **Preferred:** use GitHub's private reporting flow at
  <https://github.com/open1s/bos/security/advisories/new>

Include enough detail to reproduce: affected crate or binding, version or
commit, a description of the impact, and a proof of concept if you have one.

## What to Expect

- We aim to acknowledge a report within 3 business days.
- We will confirm the issue, assess its severity, and keep you updated on the
  fix and disclosure timeline.
- With your permission, we will credit you in the advisory once a fix ships.

## Supported Versions

The project is pre-1.0 at the crate level and ships as the `brainos` packages.
Security fixes land on the latest released version; there is no long-term
support for older releases.

| Version | Supported |
| ------- | --------- |
| latest release | :white_check_mark: |
| older releases | :x: |

## Scope

In scope: the code in this repository — the Rust workspace crates (`agent`,
`bus`, `config`, `logging`, `qserde`, `react`, `resource`), the Python (`nbos`)
and Node.js (`jsbos`) bindings, and the GitHub Actions workflows.

Out of scope: vulnerabilities in third-party dependencies are usually tracked by
Dependabot and should be reported upstream; reports that only affect an
unsupported version; and issues that require a party already holding local
filesystem or process access.
