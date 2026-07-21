import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const featureRoot = join(process.cwd(), 'src', 'features');

function sourceFiles(root: string): string[] {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = join(root, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(?:ts|tsx)$/u.test(entry.name) ? [path] : [];
  });
}

describe('experience contract guards', () => {
  const sources = sourceFiles(featureRoot)
    .filter((path) => !path.includes('.test.') && !path.includes('.stories.'))
    .map((path) => ({ path, text: readFileSync(path, 'utf8') }));

  it('uses only canonical generated contracts and the shared client', () => {
    const forbidden = [
      /\b(?:fetch|globalThis\.fetch|window\.fetch)\s*\(/u,
      /\b(?:createApiClient|newApiClient|ApiClientFactory|ReplayAdapter)\b/u,
      /\/api\/generated\/(?:operations|client)/u,
      /api\/sessions/u,
      /api\/runs/u,
      /StoredEventItem/u,
      /TaskEventPayload/u,
      /\/api\/v1\/workspaces\/[^'"`]*(?:files|diff|changes)/u,
      /\b(?:absolute_root|absolute_path|root_path)\b/u,
      /\b(?:interface|type|class)\s+(?:ContextSnapshot|ReviewSnapshot|AgentThread|AnnotationDraft)\b/u,
    ];
    for (const source of sources) {
      for (const pattern of forbidden) expect(source.text, source.path).not.toMatch(pattern);
      for (const line of source.text.split('\n').filter((value) => value.includes('/api/client'))) {
        expect(line, source.path).toMatch(
          /^import \{ (?:WebApiError|webApi)(?:, (?:WebApiError|webApi))* \} from ['"][^'"]+\/api\/client['"];$/u,
        );
      }
    }
  });

  it('hands Chat and Context the same typed relative-file callback', () => {
    const handoff = sources.find(({ path }) => path.endsWith('createExperienceSlots.tsx'));
    expect(handoff?.text).toContain('chat: { onOpenFile }');
    expect(handoff?.text).toContain('context: { onOpenFile }');
    expect(handoff?.text).toContain('RelativeFileNavigation');

    const fileConsumers = sources.filter(({ text }) => text.includes('dataSource.file('));
    expect(fileConsumers.map(({ path }) => path)).toHaveLength(1);
    expect(fileConsumers[0]?.path).toMatch(/review\/ReviewRoute\.tsx$/u);
  });

  it('does not define alternate shell, drawer, or content ownership in the handoff', () => {
    const experienceSources = sources.filter(({ path }) =>
      path.includes(join('features', 'experience')),
    );
    const definitions =
      /\b(?:function|class|const|interface|type)\s+(?:WorkbenchShell|DesktopShell|MobileShell|Drawer|SafeMarkdown|SafeCodeBlock)\b/u;
    for (const source of experienceSources) {
      expect(source.text, source.path).not.toMatch(definitions);
    }
  });

  it('keeps platform and workspace catalog access on their distinct canonical operations', () => {
    const settings = sources.find(({ path }) =>
      path.endsWith(join('settings', 'SettingsRoute.tsx')),
    );
    const guide = sources.find(({ path }) => path.endsWith(join('guide', 'GuideRoute.tsx')));
    const context = sources.find(({ path }) => path.endsWith(join('context', 'ContextPanel.tsx')));
    expect(settings?.text).toContain('api.catalog.platform()');
    expect(guide?.text).toContain('api.catalog.platform()');
    expect(context?.text).toContain('webApi.catalog.workspace(');
    expect([settings?.text, guide?.text, context?.text].join('\n')).not.toContain(
      '/api/v1/catalog',
    );
  });
});
