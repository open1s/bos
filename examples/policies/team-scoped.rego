# Team-scoped policy: named agents get full access; everyone else is denied.
# Extend the `team` set with the client certificate CNs you want to permit.
package policy

team := {"alice", "bob", "carol","agent1", "agent2", "agent3"}
actions := {"read","list", "status", "subscribe", "open", "resolve", "write", "remove", "spawn", "kill"}

allow if {
    input.agent in team
    input.action in actions
}