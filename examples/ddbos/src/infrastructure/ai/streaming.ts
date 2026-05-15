import { Agent } from '@open1s/ezbos';

export interface StreamingCallbacks {
  onThinking?: (text: string) => void;
  onText?: (text: string) => void;
  onToolCall?: (name: string) => void;
  onToolResult?: () => void;
  onDone?: () => void;
  onError?: (error: Error) => void;
}

export async function streamAgent(
  agent: Agent,
  prompt: string,
  callbacks: StreamingCallbacks = {},
): Promise<string> {
  const textParts: string[] = [];

  await agent.stream(prompt, (token: any) => {
    switch (token.type) {
      case 'ReasoningContent':
        if (callbacks.onThinking) callbacks.onThinking(token.text);
        break;
      case 'Text':
        textParts.push(token.text);
        if (callbacks.onText) callbacks.onText(token.text);
        break;
      case 'ToolCall':
        if (callbacks.onToolCall) callbacks.onToolCall(token.name);
        break;
      case 'ToolResult':
        if (callbacks.onToolResult) callbacks.onToolResult();
        break;
      case 'Done':
        if (callbacks.onDone) callbacks.onDone();
        break;
      case 'Error':
        if (callbacks.onError) callbacks.onError(new Error(token.error));
        break;
    }
  });

  return textParts.join('');
}

export async function streamAgentCollect(
  agent: Agent,
  prompt: string,
): Promise<string> {
  const tokens = await agent.streamCollect(prompt);
  return tokens
    .filter((t: any) => t.type === 'Text')
    .map((t: any) => t.text)
    .join('');
}
