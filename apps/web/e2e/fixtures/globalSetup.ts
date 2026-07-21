import { execFile } from 'node:child_process';
import { rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { promisify } from 'node:util';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(import.meta.dirname, '../../../..');
const webRoot = resolve(repositoryRoot, 'apps/web');

async function run(command: string, args: string[], cwd: string): Promise<void> {
  try {
    await execFileAsync(command, args, {
      cwd,
      env: process.env,
      maxBuffer: 10 * 1024 * 1024,
    });
  } catch (error) {
    const output =
      typeof error === 'object' && error !== null
        ? [Reflect.get(error, 'stdout'), Reflect.get(error, 'stderr')]
            .filter((value): value is string => typeof value === 'string')
            .join('\n')
        : '';
    throw new Error(`${error instanceof Error ? error.message : String(error)}\n${output}`, {
      cause: error,
    });
  }
}

export default async function globalSetup(): Promise<void> {
  if (process.env.KUKU_E2E_BINARY !== undefined) return;
  await rm(resolve(webRoot, 'dist'), { force: true, recursive: true });
  await run(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['run', 'build'], webRoot);
  await run(
    'cargo',
    ['build', '-p', 'kuku-app', '--features', 'embedded-web-assets,test-scenarios'],
    repositoryRoot,
  );
}
