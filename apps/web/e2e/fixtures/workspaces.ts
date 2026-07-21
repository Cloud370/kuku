import { execFile } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { promisify } from 'node:util';
import { join } from 'node:path';

const execFileAsync = promisify(execFile);
const GIT_ENV = {
  ...process.env,
  GIT_AUTHOR_DATE: '2024-01-02T03:04:05Z',
  GIT_AUTHOR_EMAIL: 'e2e@example.invalid',
  GIT_AUTHOR_NAME: 'Kuku E2E',
  GIT_COMMITTER_DATE: '2024-01-02T03:04:05Z',
  GIT_COMMITTER_EMAIL: 'e2e@example.invalid',
  GIT_COMMITTER_NAME: 'Kuku E2E',
};

export interface WorkspaceFixtures {
  readonly registrationRoot: string;
  readonly gitWorkspace: string;
  readonly plainWorkspace: string;
}

export async function createWorkspaces(registrationRoot: string): Promise<WorkspaceFixtures> {
  const gitWorkspace = join(registrationRoot, 'git-workspace');
  const plainWorkspace = join(registrationRoot, 'plain-workspace');
  await mkdir(join(gitWorkspace, '.agents', 'skills', 'status'), { recursive: true });
  await mkdir(join(gitWorkspace, 'src'), { recursive: true });
  await mkdir(plainWorkspace, { recursive: true });
  await writeFile(
    join(gitWorkspace, '.agents', 'skills', 'status', 'SKILL.md'),
    '---\nname: status\ndescription: Inspect status implementation\n---\n\nInspect status implementation and report facts.\n',
  );
  await writeFile(
    join(gitWorkspace, 'src', 'lib.rs'),
    'pub fn status() -> &\'static str {\n    "ready"\n}\n',
  );
  await writeFile(join(gitWorkspace, 'src', 'main.ts'), 'export const answer = 41;\n');
  await writeFile(join(plainWorkspace, 'notes.txt'), 'plain workspace fixture\n');

  await execFileAsync('git', ['init', '--initial-branch=main'], {
    cwd: gitWorkspace,
    env: GIT_ENV,
  });
  await execFileAsync('git', ['add', '.'], { cwd: gitWorkspace, env: GIT_ENV });
  await execFileAsync('git', ['commit', '-m', 'fixture baseline'], {
    cwd: gitWorkspace,
    env: GIT_ENV,
  });
  await execFileAsync('git', ['checkout', '-b', 'feature/ui'], { cwd: gitWorkspace, env: GIT_ENV });
  await writeFile(join(gitWorkspace, 'src', 'main.ts'), 'export const answer = 42;\n');

  return { registrationRoot, gitWorkspace, plainWorkspace };
}
