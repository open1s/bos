export type ContradictionType = 'technical' | 'physical';

export const CONTRADICTION_TYPES: ReadonlySet<ContradictionType> = new Set([
  'technical',
  'physical',
]);

export function isContradictionType(value: string): value is ContradictionType {
  return CONTRADICTION_TYPES.has(value as ContradictionType);
}
