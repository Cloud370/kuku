import type { ContextCatalog, ContextSnapshot, SkillCatalogEntry } from '../../api/generated';
import type { ContextSectionKey } from './ContextPanel';

export interface DiscoverableSkill extends SkillCatalogEntry {
  truth: 'Discoverable';
}

export interface ContextViewModel {
  selectedRequest: ContextSnapshot['selected_request'];
  requestHistory: ContextSnapshot['request_history'];
  requestHistoryTruncated: boolean;
  sections: ContextSnapshot['sections'];
  nextRequestBase: ContextSnapshot['next_request_base'];
  discoverable: ContextSnapshot['discoverable'] & { skills: DiscoverableSkill[] };
  staged: SkillCatalogEntry[];
  usage: {
    thisRequest: ContextSnapshot['usage']['this_request'];
    thisTask: ContextSnapshot['usage']['this_task'];
  };
  health: ContextSnapshot['health'];
  warnings: ContextSnapshot['warnings'];
  exactRequest: ContextSnapshot['exact_request'];
  exactPayloadHash: ContextSnapshot['exact_payload_hash'];
}

export function selectContextView(
  snapshot: ContextSnapshot,
  catalog: ContextCatalog,
  stagedSkillIds: string[],
): ContextViewModel {
  const staged = stagedSkillIds.flatMap((skillId) => {
    const entry = catalog.skills.find((skill) => skill.skill_id === skillId);
    return entry === undefined ? [] : [entry];
  });
  return {
    selectedRequest: snapshot.selected_request,
    requestHistory: snapshot.request_history,
    requestHistoryTruncated: snapshot.request_history_truncated,
    sections: {
      skills: snapshot.sections.skills,
      instructions: snapshot.sections.instructions,
      memory: snapshot.sections.memory,
      conversation: snapshot.sections.conversation,
      observations: snapshot.sections.observations,
      agents: snapshot.sections.agents,
      capabilities: snapshot.sections.capabilities,
    },
    nextRequestBase: snapshot.next_request_base,
    discoverable: {
      ...snapshot.discoverable,
      skills: catalog.skills.map((skill) => ({ ...skill, truth: 'Discoverable' as const })),
    },
    staged,
    usage: {
      thisRequest: snapshot.usage.this_request,
      thisTask: snapshot.usage.this_task,
    },
    health: snapshot.health,
    warnings: snapshot.warnings,
    exactRequest: snapshot.exact_request,
    exactPayloadHash: snapshot.exact_payload_hash,
  };
}

export function isWorkspaceRelativePath(path: string): boolean {
  if (path.length === 0 || path.includes('\\') || path.includes('\0')) return false;
  if (path.startsWith('/') || /^[A-Za-z]:/.test(path)) return false;
  const segments = path.split('/');
  return segments.every((segment) => segment.length > 0 && segment !== '.' && segment !== '..');
}

export function defaultOpenSections(
  snapshot: ContextSnapshot,
  stagedSkillIds: string[],
): ContextSectionKey[] {
  const sections: ContextSectionKey[] = [];
  if (stagedSkillIds.length > 0) sections.push('staged');
  sections.push('skills');
  const observationWarning =
    snapshot.health.source_drift_count > 0 ||
    snapshot.health.truncated_observation_count > 0 ||
    snapshot.warnings.some((warning) =>
      ['observation_truncated', 'source_drift', 'source_inaccessible'].includes(warning.code),
    );
  if (observationWarning) sections.push('observations');
  if (snapshot.warnings.length > 0) sections.push('health');
  return sections;
}
