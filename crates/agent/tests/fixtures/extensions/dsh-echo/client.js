// Fixture client entry. The shim declares the panel but does not render it.
export function apply(ctx) {
  ctx.slots.register({ slot: 'conversation.composer.dock' });
}
