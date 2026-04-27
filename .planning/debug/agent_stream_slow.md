# Debug Session: agent_stream_demo.cjs - streaming and thinking

**Status**: RESOLVED
**Started**: 2026-04-26
**Resolved**: 2026-04-27

## Root Causes Fixed

### 1. Invalid Model Name (jsbos)
- **Issue**: Config had invalid model `nvidia/nvidia/nemotron-mini-4b-instruct` (double prefix)
- **Fix**: Changed default to valid `z-ai/glm4.7` in demo

### 2. Thinking Support Missing (pybos)
- **Issue**: Line 850 was skipping ReasoningContent tokens with `continue`
- **Fix**: Emit as JSON `{"type": "thinking", "text": "..."}`

## Test Results

### jsbos
```bash
cd crates/jsbos && node examples/agent_stream_demo.cjs
# Output: Text + ToolCall tokens stream properly
# Use SHOW_THINKING=1 to see thinking in stderr
```

### pybos
```python
stream = await agent.stream('What is 1+1?')
async for token in stream:
    # token is either plain text OR {"type": "thinking", "text": "..."}
```
Tokens: 17, Thinking chunks: 14 ✓

## Files Changed
- `crates/jsbos/examples/agent_stream_demo.cjs` - model fix
- `crates/pybos/src/agent.rs:850` - thinking emission

## Status
- [x] jsbos streaming: Working
- [x] jsbos model: Fixed
- [x] pybos thinking: Working