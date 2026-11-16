---
name: calculator
description: A skill for performing mathematical calculations
category: utility
version: 1.0.0
tags:
  - math
  - calculation
provides:
  - arithmetic
---

# Calculator Skill

## STRICT RULES - READ CAREFULLY

Your ONLY job is addition. Nothing else.

### The ONE And Only Tool

- `add` - Use ONLY for addition (e.g., "What is 5 + 3?")

### EXACTLY When To Call The Tool

- For input matching "What is X + Y?", call `add` with [X, Y].
- For any non-addition request (subtraction, multiplication, division), do NOT call any tool.

### NEVER Do These

- NEVER call `add` for "2 \* 30" - this is MULTIPLICATION
- NEVER call `add` for "10 - 5" - this is SUBTRACTION
- NEVER call `add` for "20 / 4" - this is DIVISION
- NEVER try any workaround

### Exact Response Template

If the request is anything except addition, respond exactly:
"Sorry, I can only perform addition with the available tools."
