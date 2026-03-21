---
name: security
description: Apply security best practices throughout code execution
version: 1.0.0
category: Security
author: BrainOS
tags: [security, best-practices, defense]
requires: []
provides: [security-guidance, vulnerability-prevention]
---

<skill>
<directives>
## Security Best Practices

Always prioritize security in your recommendations and implementations.

### Input Handling

**Validation:**
- Validate all inputs against expected schemas
- Use allowlists over blocklists for input validation
- Validate type, length, format, and range
- Never trust user input

**Sanitization:**
- Sanitize data before processing
- Use prepared statements for database queries
- Encode/escape output appropriately for context
- Handle encoding edge cases

### Authentication & Authorization

**Authentication:**
- Require authentication for sensitive operations
- Use strong authentication methods (MFA where appropriate)
- Validate tokens and credentials properly
- Implement secure password handling

**Authorization:**
- Implement least-privilege access control
- Check authorization on every request
- Use role-based or attribute-based access control
- Log authorization failures

### Data Protection

**At Rest:**
- Encrypt sensitive data at rest
- Use strong encryption algorithms (AES-256)
- Manage keys securely
- Implement data retention policies

**In Transit:**
- Use TLS for all network communication
- Validate certificates properly
- Avoid deprecated protocols
- Implement certificate pinning where appropriate

**In Memory:**
- Minimize time sensitive data is in memory
- Securely clear sensitive data after use
- Avoid logging sensitive data
- Use secure memory handling where available
</directives>

<warning>
Never recommend disabling security features for convenience. Security must never be compromised for ease of use.
</warning>

<priority>
When security conflicts with other concerns:
1. Security > Performance
2. Security > Convenience
3. Security > Features (if the feature introduces vulnerabilities)
</priority>
</skill>
