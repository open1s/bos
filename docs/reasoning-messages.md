# Reasoning messages

Reasoning is model output, but it is not an ordinary assistant message. The
react session keeps it as a separate `LlmMessage::AssistantReasoning` variant so
the semantic boundary between thinking and the answer survives, and so provider
adapters can decide what (if anything) to put back on the wire.

## Turn-local rule

Reasoning belongs to the turn that produced it. A tool loop inside that turn
must replay the reasoning blocks the same provider emitted (Anthropic's signed
thinking, a Responses reasoning item, DeepSeek's `reasoning_content`); a later
turn must never receive them. The engine drops every prior-turn
`AssistantReasoning` before a turn's first request
(`ReactSession::clear_reasoning`), and BSH does not seed reasoning from the
transcript at all. There is no persisted thinking summary: the old
`thinking_context` feature and its `ThinkingSnapshot` are gone.

## Message contract

~~~rust
pub enum LlmMessage {
    // ...
    AssistantReasoning {
        content: String,
        provider_state: Option<ReasoningState>,
    },
}

pub struct ReasoningState {
    pub provider: String,   // "anthropic", "openai-responses", "openai-chat"
    pub payload: Value,     // opaque provider payload, preserved verbatim
}
~~~

`content` is the raw reasoning text. `provider_state` is optional: a protocol
that signs or encrypts reasoning state puts its opaque payload there. One
message carries one provider block; the engine accumulates a block at a time so
several signed / redacted / encrypted blocks keep their own state.

## Replay policy (inside the turn only)

| Provider wire | What is replayed | Field / item |
|---|---|---|
| OpenAI Chat Completions | nothing | none |
| DeepSeek Chat Completions | the turn's reasoning text | `reasoning_content` on the assistant row |
| OpenRouter Chat Completions | text + structured details | `reasoning` + `reasoning_details` |
| Anthropic Messages | signed thinking and redacted thinking | `thinking` / `redacted_thinking` blocks |
| OpenAI / DeepSeek Responses | the reasoning item, verbatim | `reasoning` input item (id, encrypted_content, content, summary, signature) |

Anthropic extended thinking is requested from the reader's effort level:
`low` / `medium` / `high` map to a `budget_tokens` and the completion cap is
raised above it. The Responses request asks for `reasoning.summary = "auto"`
and `include: ["reasoning.encrypted_content"]` so the item is replayable.

`Off` is explicit rather than an omitted field: DeepSeek and Anthropic get
`thinking: {"type": "disabled"}`, OpenRouter gets
`reasoning: {"enabled": false}`, and a Responses endpoint that accepts an off
value gets `effort: "none"` (OpenAI's own Responses API omits the block, which
has no portable off value).

A provider switch cannot replay one vendor's state into another: the engine
clears prior turns before every request, so only the current turn's blocks are
ever serialized, and it was produced by the provider this turn is talking to.

## Where it is captured

- **Non-streaming**: `message.reasoning_content` / `reasoning` is pushed as
  `AssistantReasoning` before the assistant text; an Anthropic signature arrives
  in the message extension and is attached as `ReasoningState`.
- **Streaming**: reasoning chunks accumulate per block. A
  `StreamToken::ReasoningState` closes the block (Anthropic's
  `signature_delta`, a Responses `output_item.done`); a state with no text is
  its own block (redacted thinking). The engine pushes one
  `AssistantReasoning` per block, before the visible text and tool calls.
- **BSH transcript**: reasoning text and its display timeline (`TurnPart`) are
  stored for the reader. No provider state is persisted or replayed: the next
  turn seeds only the user/assistant text and the tool traffic.
