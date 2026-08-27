# Path-scoped policy: agents may only reach resources under an allowed
# filesystem prefix (here `/Users/…`). Any other path is denied.
package policy

allow if {
    startswith(input.uri, "file:///Users/")
}

allow if {
    startswith(input.uri, "folder:///Users/")
}