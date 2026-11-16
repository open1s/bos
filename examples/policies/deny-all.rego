# Deny everything (locked down). Since no `allow` rule is defined, every
# request is denied by default. Only admins (via the `admins` list in the
# policy document) can update policy; nothing else is allowed.
package policy

# Intentionally no `allow` rules -> deny-by-default.