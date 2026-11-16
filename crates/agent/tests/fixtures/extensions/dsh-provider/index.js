// The imperative host entry a real DSH model-provider package ships. This
// build has no JavaScript runtime, so the entry is read for its markers and
// refused; the declarative providers/*.json beside it is what actually loads.
export const name = '@local/dsh-zen';
export const inject = ['llm'];

export function apply(ctx, config = {}) {
  // The adapter contract: providerInfo/listModels/resolveModel/stream.
  const adapter = {
    providerInfo: (provider) => ({ id: provider, name: 'Zen' }),
    listModels: async () => [],
  };
  return ctx.llm.registerAdapter(['zen'], adapter);
}
