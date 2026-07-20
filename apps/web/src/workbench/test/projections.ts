import taskProjectionJson from '../../api/generated/fixtures/task_projection.json';
import type { ContextCatalog, TaskPage, TaskProjection, WorkspacePage } from '../../api/generated';

export const fixtureTaskId = 'tsk_000000000000000000000001';
export const fixtureWorkspaceId = 'wsp_000000000000000000000001';

export function readyProjection(): TaskProjection {
  const value = structuredClone(taskProjectionJson) as TaskProjection;
  value.task.task_id = fixtureTaskId;
  value.task.workspace_id = fixtureWorkspaceId;
  value.selected_tier_id = 'tier:balanced';
  return value;
}

export function fixtureCatalog(): ContextCatalog {
  return {
    agents: [],
    api_version: 1,
    revision: 'revision-catalog',
    skills: [
      {
        description: 'Review Rust changes',
        name: 'rust-review',
        skill_id: 'skill:project:rust-review',
        source: { id: 'src-rust', relative_path: null, scope: 'project' },
      },
    ],
    tiers: [
      {
        tier: {
          is_default: true,
          label: 'Balanced',
          model: 'fixture-model',
          provider: 'fixture-provider',
          purpose: 'General work',
          think: null,
          tier_id: 'tier:balanced',
        },
      },
    ],
    tools: [],
  };
}

export function fixtureTaskPage(): TaskPage {
  return { api_version: 1, items: [readyProjection().task], next_cursor: null };
}

export function fixtureWorkspacePage(): WorkspacePage {
  return {
    api_version: 1,
    items: [
      {
        availability: 'available',
        branch: 'feature/web',
        is_default: true,
        label: 'kuku',
        workspace_id: fixtureWorkspaceId,
      },
    ],
    server_revision: 'revision-workspaces',
  };
}
