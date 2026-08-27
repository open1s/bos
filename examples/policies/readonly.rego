# Read-only policy: agents may only inspect (read/list/status/resolve) but
# never mutate (write/remove/spawn/kill/etc).
package policy

allow if {
    input.action in {"read", "list", "status", "subscribe"}
}

# Also permit discovery operations.
allow if {
    input.action in {"open", "resolve"}
}