// The package's web client: it contributes a settings page through the DSH
// slot registry, which this build has no browser runtime for.
export function apply(ctx) {
  ctx.slots.register({ name: 'settings.section', id: 'zen' }, () => null);
}
