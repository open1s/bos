# Storage-only policy: agents may read/write files and folders, but nothing
# else (no sockets, processes, memory stores, or signals).
package policy

allow if {
    startswith(input.uri, "file://")
}

allow if {
    startswith(input.uri, "folder://")
}