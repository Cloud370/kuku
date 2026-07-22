export type ContextSectionKey =
  | 'staged'
  | 'skills'
  | 'instructions'
  | 'memory'
  | 'conversation'
  | 'observations'
  | 'agents'
  | 'discoverable'
  | 'capabilities'
  | 'usage'
  | 'health';

const defaultContextSections: ContextSectionKey[] = [
  'staged',
  'skills',
  'instructions',
  'memory',
  'conversation',
  'observations',
  'agents',
  'discoverable',
  'capabilities',
  'usage',
  'health',
];

export function resolveInitialOpenSections(values: string[]): ContextSectionKey[] {
  const saved = values.filter((value): value is ContextSectionKey =>
    defaultContextSections.includes(value as ContextSectionKey),
  );
  return saved.length > 0 ? saved : [...defaultContextSections];
}
