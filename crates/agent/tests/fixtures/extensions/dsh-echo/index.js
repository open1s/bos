// Fixture host entry. The reference registers these through the JavaScript
// host API; the shim refuses to execute this file.
export function apply(ctx) {
  ctx.tools.register({ name: 'imperative_echo' });
  ctx.commands.register({ name: 'imperative', description: 'Imperative command' });
  ctx.on('tools/result', () => {});
}
