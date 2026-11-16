// The whole payload: an imperative LLM adapter and nothing else. No
// providers/*.json, no tools/*.json - exactly the shape an upstream DSH
// package ships, because the DSH manifest has no field for any of it.
export const name = '@local/dsh-script-only';

export function apply(ctx, config = {}) {
  const adapter = {
    providerInfo: (provider) => ({ id: provider, name: 'Zen' }),
    listModels: async () => [],
  };
  return ctx.llm.registerAdapter(['zen'], adapter);
}
