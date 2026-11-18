# Reasoning messages

Reasoning is model output, but it is not an ordinary assistant message. The
react session keeps it as a separate `LlmMessage::AssistantReasoning` variant so
the semantic boundary between thinking and the answer survives, and so provider
adapters can decide what (if anything) to put back on the wire.

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
    pub provider: String,   // "anthropic", "openai-responses", ...
    pub payload: Value,     // opaque provider payload, preserved verbatim
}
~~~

`content` is the raw reasoning text. `provider_state` is optional: a protocol
that signs or encrypts reasoning state (Anthropic thinking signatures, Responses
reasoning items) puts its opaque payload there. The engine stores both; it never
rewrites either.

## Replay policy

| Item | Sent to the next LLM? |
|---|---|
| Normal `Assistant` text | Yes, according to the conversation-history policy |
| Model-returned `AssistantReasoning` | Not by default |
| Provider-required reasoning state | Only by the adapter that recognizes its own `provider` |
| Thinking summary | `Assistant`-role context on demand; never a system message |

| Context source | Wire role | Precedence | Trust | Framing |
|---|---|---|---|---|
| Thinking summary | `assistant` | Medium | non-authoritative | `<execution_context source="thinking-summary" trust="non-authoritative" precedence="medium">` |
| Retrieved memory | `user` | Medium | untrusted-data | `<untrusted_context source="retrieved-memory" trust="untrusted-data" precedence="medium">` |
| Quoted/untrusted content | `user` | Low | untrusted-data | `<untrusted_context ... precedence="low">` |

The four priorities are `Highest`, `High`, `Medium`, `Low`. Role says who
the message belongs to, precedence says which side wins a conflict, and trust says
how far the content can be believed; the three are independent. `System` is
Highest; the current user request is High; normal assistant history, the
thinking summary and retrieved memory are Medium; raw reasoning and tool traffic
are Low.

The summary is assistant-side task-continuation state, not raw reasoning and not
policy. Retrieved memory is historical reader-side data, not a new request. Both
are framed immediately before the current input and never enter the system
prompt. A summary does not use `AssistantReasoning`; that type remains reserved
for provider-native reasoning blocks.

The default is deliberate: the Chat Completions adapter skips
`AssistantReasoning` entirely. A provider switch therefore cannot replay an
Anthropic signature into an OpenAI-compatible endpoint or vice versa.

## Adapter mapping

- **Chat Completions / DeepSeek / OpenAI-compatible**
  - Reasoning is history-only. No `reasoning_content` is written back.
- **Anthropic Messages**
  - A message with `provider: "anthropic"` and
    `payload.type == "thinking"` replays as a `thinking` block with its
    signature.
  - `payload.type == "redacted_thinking"` replays as a `redacted_thinking`
    block.
  - Consecutive assistant blocks are merged into one assistant message, so a
    thinking block and its `tool_use` blocks stay in the same turn.
  - Reasoning without a signature is not sent; a signature cannot be invented.
- **OpenAI Responses**
  - A message with `provider: "openai-responses"` and a payload `id` replays as
    a `reasoning` input item, carrying `encrypted_content` when present.
  - Visible reasoning without Responses state is history-only.

## Where it is captured

- **Non-streaming**: `message.reasoning_content` is pushed as
  `AssistantReasoning` before the assistant text.
- **Streaming**: reasoning chunks accumulate per step; a
  `StreamToken::ReasoningState` carries the provider's opaque state. At the step
  boundary the engine pushes one `AssistantReasoning` message, before the
  visible text and tool calls, then clears both buffers.
- **Anthropic SSE**: `signature_delta` and `redacted_thinking` blocks emit
  `ReasoningState`.
- **Responses SSE**: reasoning `output_item.added`/`done` events emit
  `ReasoningState` when they carry an item id or encrypted content.
- **BSH transcript**: `AgentEvent::ReasoningState` persists the states in
  `MessageMeta.reasoning_states`. `Runner::seed_session` splits the reasoning
  text by its `TurnPart::Thinking` lengths and replays one `AssistantReasoning`
  per block, each with its own state, before the assistant text.

## Existing behaviour preserved

- Reasoning is still streamed to the reader as `StreamToken::ReasoningContent`.
- Thinking summaries remain a separate, optional context channel.
- The engine's token estimate counts reasoning only when provider state is
  present, because that is the only case an adapter may send it.
- Compaction does not feed raw reasoning to the summarizer; deriving a compact
  summary is the thinking-summary feature's job, not a license to replay the
  chain of thought.

## Timeline pairing

The BSH transcript stores the reasoning text once and its provider states in
`MessageMeta.reasoning_states`. The `TurnPart::Thinking` lengths split the text
back into blocks, so a turn with several signed thinking blocks replays each
block with its own signature. An older row with no timeline replays its whole
reasoning text with the first state; the transcript never invents a block the
provider did not produce.
