---
name: code-analysis
description: Analyze code for quality, security, and performance issues
version: 1.0.0
category: Code
author: BrainOS
tags: [quality, security, performance, review]
requires: []
provides: [code-review, security-audit, performance-analysis]
---

<skill>
<directives>
## Code Analysis Checklist

When reviewing or analyzing code, systematically check for:

### 1. Security Vulnerabilities

**Input Handling:**
- SQL injection risks in database queries
- XSS vulnerabilities in output rendering
- Command injection in shell executions
- Path traversal in file operations

**Authentication & Authorization:**
- Missing authentication checks
- Improper authorization boundaries
- Session management issues
- Credential handling problems

**Data Protection:**
- Sensitive data exposure in logs
- Insecure data storage
- Missing encryption for sensitive data
- Improper secret management

### 2. Performance Issues

**Database:**
- N+1 query patterns
- Missing indexes on frequently queried columns
- Inefficient query structures
- Missing pagination for large datasets

**Memory & Resources:**
- Memory leaks
- Unnecessary allocations
- Missing resource cleanup
- Connection pool exhaustion

**Algorithms:**
- Inefficient algorithmic complexity
- Unnecessary loops or iterations
- Redundant computations
- Missing caching opportunities

### 3. Code Quality

**Readability:**
- Poor naming conventions
- Deeply nested code
- Long functions or methods
- Missing or misleading comments

**Maintainability:**
- Code duplication
- Tight coupling
- Missing error handling
- Inconsistent style

**Testing:**
- Missing unit tests
- Missing edge case coverage
- Brittle or flaky tests
- Missing integration tests
</directives>

<references>
See references/checklist.md for a detailed review checklist you can use during code reviews.
</references>

<workflow>
1. Start with security analysis (highest priority)
2. Check for performance issues
3. Review code quality
4. Document findings with severity levels
5. Provide actionable recommendations
</workflow>
</skill>
