const LANGUAGE_BY_EXTENSION: Record<string, string> = {
  bash: 'bash',
  js: 'javascript',
  json: 'json',
  md: 'markdown',
  markdown: 'markdown',
  mjs: 'javascript',
  rs: 'rust',
  sh: 'bash',
  ts: 'typescript',
  tsx: 'typescript',
  typescript: 'typescript',
};

export function resolveReviewLanguage(pathOrLanguage: string | null): string {
  const value = pathOrLanguage?.trim().toLowerCase();
  if (!value) return 'plaintext';
  const leaf = value.split('/').at(-1) ?? value;
  const extension = leaf.includes('.') ? (leaf.split('.').at(-1) ?? '') : leaf;
  return LANGUAGE_BY_EXTENSION[extension] ?? 'plaintext';
}
