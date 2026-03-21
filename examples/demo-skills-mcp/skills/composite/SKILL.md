---
name: composite
description: Demonstrates skill composition using multiple dependent skills
version: 1.0.0
category: Domain
author: BrainOS
tags: [composition, example, comprehensive]
requires: [code-analysis, security]
provides: [comprehensive-code-review]
---

<skill>
<directives>
## Comprehensive Code Review

This skill combines `code-analysis` and `security` to provide thorough code reviews.

### Review Process

When reviewing code, always follow this sequence:

1. **Apply Security Analysis First**
   - Use the security skill to identify vulnerabilities
   - Check authentication and authorization
   - Verify data protection measures
   - Security issues take highest priority

2. **Run Code Analysis**
   - Apply the code-analysis checklist
   - Check for performance issues
   - Review code quality metrics
   - Identify maintainability concerns

3. **Synthesize Findings**
   - Combine findings from both skills
   - Prioritize by severity and impact
   - Provide actionable recommendations
   - Consider both functional quality and security posture

### Priority Order

When issues conflict or compete for attention:
- Critical security vulnerabilities → Immediate fix required
- High security risks → Fix before deployment
- Performance issues → Fix if significant impact
- Code quality → Fix during refactoring cycles

### Output Format

Structure your review as:
```
## Security Findings
- [Critical] ...
- [High] ...
- [Medium] ...

## Code Quality Findings
- [Performance] ...
- [Maintainability] ...
- [Style] ...

## Recommendations
1. ...
2. ...
```
</directives>

<composition>
This skill depends on:
- **code-analysis**: Provides the review checklist and quality metrics
- **security**: Provides security analysis and vulnerability detection

When loaded together, ensure all directives from both skills are applied.
The composite skill orchestrates the application of both skills.
</composition>

<note>
This skill demonstrates how skills can depend on and compose with each other.
The `requires` field in the frontmatter declares these dependencies.
</note>
</skill>
