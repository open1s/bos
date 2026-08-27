# Rego Policy Examples

Ready-to-use Rego authorization policies for `rex serve --policy <file>.rego`.

A policy decides, for each request, whether it is **allowed** or **denied**.
The regorus engine evaluates a rule named `allow`; if it produces no result,
the request is denied (deny-by-default).

## Input

Each authorization request provides the following `input`:

| Field    | Type   | Example                        |
|----------|--------|--------------------------------|
| `agent`  | string | `agent1` (client certificate CN) |
| `uri`    | string | `file:///tmp/notes.txt`        |
| `action` | string | `read`, `write`, `spawn`, …    |

## Examples

| File | Behavior |
|------|----------|
| [`allow-all.rego`](allow-all.rego) | Allow every request (permissive default). |
| [`deny-all.rego`](deny-all.rego) | Deny every request (locked down). |
| [`readonly.rego`](readonly.rego) | Permit only inspection actions (`read`, `list`, `status`, …). |
| [`storage-only.rego`](storage-only.rego) | Permit only `file://` and `folder://` resources. |
| [`path-scoped.rego`](path-scoped.rego) | Permit only resources under a fixed filesystem prefix. |
| [`team-scoped.rego`](team-scoped.rego) | Permit only named agents (a fixed allow-list). |

## Usage

```bash
rex serve \
  --register "folder:///srv/data" \
  --policy examples/policies/readonly.rego \
  --ca certs/ca.pem --cert certs/server.pem --key certs/server.key
```

## Writing your own

Start from the closest example and adjust. The core shape is:

```rego
package policy

allow if {
    # conditions that must all hold
    input.agent != "blocked"
    startswith(input.uri, "file://")
}
```

Useful rego built-ins: `startswith`, `endswith`, `contains`, `regex.match`,
`in` (set/array membership), `some`, `not`, `and`, `or` (as separate rules).