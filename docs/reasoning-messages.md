# Reasoning messages

Reasoning is model output, but it is not an ordinary assistant message. The
react session keeps it as a separate `LlmMessage::AssistantReasoning` variant so
the semantic boundary between thinking and the answer survives, and so provider
adapters can decide what (if anything) to put back on the wire.

## Replay scope

Reasoning is turn-local by default. A tool loop inside the turn that produced a
block must replay it to the same provider (Anthropic's signed thinking, a
Responses reasoning item, DeepSeek's `reasoning_content`); a later turn must
never receive it. The engine drops every prior-turn `AssistantReasoning`
before a turn's first request (`ReactSession::clear_reasoning`).

DeepSeek is the exception the provider reports itself:
`LlmClient::replays_reasoning_across_turns` returns `true` for a DeepSeek
vendor (and for a generic client pointed at `api.deepseek.com`). Its thinking
mode documents that every earlier turn's chain of thought comes back once a
request carries `tools`, and its `/responses` validator enforces the per-step
lead described below with the error
`The reasoning_text in the thinking mode must be passed back to the API`. The
engine then skips `clear_reasoning`, and BSH seeds the transcript's stored
reasoning as `AssistantReasoning { provider_state: None }` rows before that
turn's text and tool traffic. A strict Responses endpoint has no encrypted
state to replay, so the request builder synthesizes a `reasoning_text` input
item from that text, or the accepted one-space placeholder for a step that has
none.

The wire order matters as much as the content. DeepSeek's `/responses` reads
one step — a run of the model's reasoning, what it said and the calls it made,
between the user's last message or a tool output and the next — as one assistant
message whose thinking is the item the step starts with. A step that makes a
call, or that follows a tool output, must therefore lead with a reasoning item:
`[message, reasoning, call]` and `[output, message]` are refused. The Responses
adapter moves a step's own first reasoning item ahead of the rest, and gives a
step with none a one-space `reasoning_text` placeholder. The Chat adapter does
the same for a tool-making assistant row, or one that follows a tool result,
with `reasoning_content`. Both only for the strict DeepSeek endpoint;
OpenAI-style endpoints keep the order they emitted.

`clear_reasoning` is a required trait method, not a defaulted no-op: a custom
session that keeps history must remove every `AssistantReasoning` message and
its provider state before the engine starts a turn, and must also be correct
when the engine skips the call for a provider that needs full pass-back. There
is no persisted thinking summary: the old `thinking_context` feature and its
`ThinkingSnapshot` are gone.

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
| OpenAI-compatible Chat Completions (default; NVIDIA NIM and custom gateways included) | the turn's reasoning text | `reasoning_content` on the assistant row |
| Custom OpenAI-compatible gateway using `reasoning` | the turn's reasoning text | `reasoning` on the assistant row |
| Custom OpenAI-compatible gateway using `reasoning_details` | text + structured details | `reasoning` + `reasoning_details` |
| DeepSeek Chat Completions | every turn's reasoning text | `reasoning_content` on the assistant row |
| OpenRouter Chat Completions | text + structured details | `reasoning` + `reasoning_details` |
| Anthropic Messages | signed thinking and redacted thinking | `thinking` / `redacted_thinking` blocks |
| OpenAI Responses | the reasoning item, verbatim | `reasoning` input item (id, encrypted_content, content, summary, signature) |
| DeepSeek Responses | every turn's reasoning text | `reasoning` input item with `reasoning_text` content parts (no id, no encrypted state) |

OpenAI's own chat API has no reasoning field, so its requests never carry
one: a reasoning block only exists when the model emitted it in this turn.
A generic OpenAI-compatible client learns the custom gateway's dialect from its
own response - `reasoning_content`, `reasoning` or `reasoning_details` - and
replays the next step in the same field, so an arbitrary gateway does not have
to be named in advance. `ReasoningReplay::None` is the explicit opt-out for an
endpoint whose schema rejects the field.

Anthropic extended thinking is requested from the reader's effort level:
`low` / `medium` / `high` map to a `budget_tokens` and the completion cap is
raised above it. The Responses request asks for `reasoning.summary = "auto"`
and `include: ["reasoning.encrypted_content"]` so the item is replayable.

`Off` is explicit rather than an omitted field: DeepSeek and Anthropic get
`thinking: {"type": "disabled"}`, OpenRouter gets
`reasoning: {"enabled": false}`, and a Responses endpoint that accepts an off
value gets `effort: "none"` (OpenAI's own Responses API omits the block, which
has no portable off value). The generic OpenAI-compatible default replays
`reasoning_content` but has no portable off field, so an explicit Off only omits
the effort there: replay and the off switch are separate provider facts, and a
vendor opts into each one on its own.

A provider switch cannot replay one vendor's state into another: every vendor
only serializes the state it recognizes, a text-only block is synthesized into
a wire item only for the strict DeepSeek Responses endpoint, and every other
provider still starts a turn from a cleared session.

## Where it is captured

- **Non-streaming**: `message.reasoning_content` / `reasoning` is pushed as
  `AssistantReasoning` before the assistant text; an Anthropic signature arrives
  in the message extension and is attached as `ReasoningState`. When the
  Messages stream carried several signed or redacted blocks, the vendor writes
  `_bos_reasoning_blocks` (one `{content, state}` per block) and the engine
  pushes one `AssistantReasoning` per entry. A single block keeps the original
  `reasoning_content` + attached-state fields.
- **Streaming**: reasoning chunks accumulate per block. A
  `StreamToken::ReasoningState` closes the block (Anthropic's
  `signature_delta`, a Responses `output_item.done`); a state with no text is
  its own block (redacted thinking). The engine pushes one
  `AssistantReasoning` per block, before the visible text and tool calls.
- **BSH transcript**: reasoning text and its display timeline (`TurnPart`) are
  stored for the reader. No provider state is persisted. The next turn seeds
  the text and tool traffic, plus the reasoning text itself when the vendor
  reports that it requires full pass-back (DeepSeek).
