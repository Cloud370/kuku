import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

import { expect, test } from './fixtures/unifiedBinary';
import { firstTaskId, openAuthenticatedRoute } from './fixtures/journey';
import {
  authenticatedGet,
  contextSnapshot,
  taskProjection,
  workspacePage,
} from './fixtures/productApi';
import { completeScenario } from './fixtures/scenarioControl';

test('keeps staged Skills distinct from agent-loaded Context facts', async ({
  browser,
  request,
  unifiedBinary,
}) => {
  const taskId = await firstTaskId(request, unifiedBinary);
  await completeScenario(request, unifiedBinary);
  const agentSkillPath = '.agents/skills/status/SKILL.md';
  const agentSkillContent = await readFile(
    join(unifiedBinary.gitWorkspace, agentSkillPath),
    'utf8',
  );
  const agentSkillHash = `sha256:${createHash('sha256').update(agentSkillContent).digest('hex')}`;
  const initialContext = await contextSnapshot(request, unifiedBinary, taskId);
  expect(initialContext.sections.skills).toEqual(
    expect.arrayContaining([
      expect.objectContaining({
        content_hash: agentSkillHash,
        origin: 'agent',
        skill_id: 'skill:project:status',
        source: expect.objectContaining({ relative_path: agentSkillPath, scope: 'project' }),
      }),
    ]),
  );
  expect(initialContext.sections.skills).not.toEqual(
    expect.arrayContaining([
      expect.objectContaining({ skill_id: 'skill:project:acceptance-review' }),
    ]),
  );
  const initialProjection = await taskProjection(request, unifiedBinary, taskId);
  expect(initialProjection.loaded_skills).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ loaded_by: 'agent', skill_id: 'skill:project:status' }),
    ]),
  );
  const skillName = 'acceptance-review';
  const skillDirectory = join(unifiedBinary.gitWorkspace, '.agents', 'skills', skillName);
  await mkdir(skillDirectory, { recursive: true });
  await writeFile(
    join(skillDirectory, 'SKILL.md'),
    `---\nname: ${skillName}\ndescription: Review the acceptance workspace\n---\n\nInspect typed Context provenance.\n`,
  );
  const workspaces = await workspacePage(request, unifiedBinary);
  const git = workspaces.items.find(({ label }) => label === 'Git fixture');
  if (git === undefined) throw new Error('scenario has no Git fixture workspace');
  const catalogPath = `/workspaces/${encodeURIComponent(git.workspace_id)}/catalog?search=${skillName}`;
  await expect
    .poll(async () => {
      const response = await authenticatedGet(request, unifiedBinary, catalogPath);
      if (!response.ok()) return [];
      const catalog = (await response.json()) as { skills: Array<{ skill_id: string }> };
      return catalog.skills.map(({ skill_id }) => skill_id);
    })
    .toContain(`skill:project:${skillName}`);

  const page = await openAuthenticatedRoute(
    await browser.newContext(),
    unifiedBinary,
    `/tasks/${encodeURIComponent(taskId)}`,
  );
  await expect(page.getByRole('complementary', { name: 'Agent Context' })).toBeVisible();
  await expect(page.getByText('Loaded by Agent', { exact: true })).toBeVisible();
  await expect(page.getByText(agentSkillPath, { exact: false })).toBeVisible();
  await expect(page.getByText('Next Request preview', { exact: false })).toHaveCount(0);
  await page.getByLabel('Add Skill').click();
  await expect(page.getByRole('listbox', { name: 'Available Skills' })).toBeVisible();
  await page.getByLabel('Search Skills').fill(skillName);
  await page.getByRole('option', { name: skillName }).click();
  await expect(page.getByText('Next Request preview', { exact: true })).toBeVisible();
  await expect(page.getByText(skillName, { exact: true }).first()).toBeVisible();

  let submittedRun: { message?: string; skill_ids?: string[]; tier_id?: string } | null = null;
  page.on('request', (outgoing) => {
    if (outgoing.method() !== 'POST' || !outgoing.url().endsWith(`/tasks/${taskId}/runs`)) return;
    submittedRun = outgoing.postDataJSON() as {
      message?: string;
      skill_ids?: string[];
      tier_id?: string;
    };
  });
  await page
    .getByRole('textbox', { name: 'Message', exact: true })
    .fill('Use the staged acceptance Skill');
  await page.getByRole('button', { name: 'Send' }).click();
  await expect
    .poll(() => submittedRun)
    .toMatchObject({
      message: 'Use the staged acceptance Skill',
      skill_ids: [`skill:project:${skillName}`],
      tier_id: 'tier:e2e-balanced',
    });
  await expect
    .poll(async () =>
      (await contextSnapshot(request, unifiedBinary, taskId)).sections.skills.map(
        ({ skill_id }) => skill_id,
      ),
    )
    .toEqual(expect.arrayContaining(['skill:project:status', `skill:project:${skillName}`]));
  const context = await contextSnapshot(request, unifiedBinary, taskId);
  const selectedSkill = context.sections.skills.find(
    ({ skill_id }) => skill_id === `skill:project:${skillName}`,
  );
  expect(selectedSkill).toMatchObject({
    origin: 'you',
    skill_id: `skill:project:${skillName}`,
    source: {
      relative_path: `.agents/skills/${skillName}/SKILL.md`,
      scope: 'project',
    },
  });
  const projection = await taskProjection(request, unifiedBinary, taskId);
  expect(projection.loaded_skills).toEqual(
    expect.arrayContaining([
      expect.objectContaining({ loaded_by: 'agent', skill_id: 'skill:project:status' }),
      expect.objectContaining({ loaded_by: 'you', skill_id: `skill:project:${skillName}` }),
    ]),
  );
  await expect(page.getByText('Next Request preview', { exact: false })).toHaveCount(0);
  const skillsSection = page.getByRole('button', { name: /^Skills 2$/ });
  if ((await skillsSection.getAttribute('aria-expanded')) !== 'true') await skillsSection.click();
  await expect(page.getByText('Loaded by You', { exact: true })).toBeVisible();
  await expect(
    page.getByText(`.agents/skills/${skillName}/SKILL.md`, { exact: false }),
  ).toBeVisible();
  await expect(page.locator('body')).not.toContainText(unifiedBinary.gitWorkspace);
});
