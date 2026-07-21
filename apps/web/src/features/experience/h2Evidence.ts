export const H2_EVIDENCE = [
  {
    id: 'context-current-verified',
    story: 'experience-context--verified',
    route: '/tasks/tsk_000000000000000000000001',
    expected: ['Verified', 'Loaded by Agent'],
  },
  {
    id: 'context-history-exact',
    story: 'experience-context--historical',
    route: '/tasks/tsk_000000000000000000000001?request=req_000000000000000000000001',
    expected: ['req_000000000000000000000001', 'Exact request'],
  },
  {
    id: 'context-discoverable',
    story: 'experience-context--discoverable-only',
    route: '/tasks/tsk_000000000000000000000001',
    expected: ['Can discover', 'Discoverable'],
  },
  {
    id: 'context-agent-thread',
    story: 'experience-context--agent-thread',
    route: '/tasks/tsk_000000000000000000000001?agent=con_000000000000000000000002',
    expected: ['Read only', 'Result in main'],
  },
  {
    id: 'context-sticky-staged-desktop',
    story: 'experience-context--sticky-summary-desktop',
    route: '/tasks/tsk_000000000000000000000001',
    expected: [
      'Summary remains below header while accordions scroll',
      'Staged absent when no Skill is staged',
    ],
  },
  {
    id: 'context-sticky-staged-360',
    story: 'experience-context--sticky-summary-360',
    route: '/tasks/tsk_000000000000000000000001',
    expected: [
      'Summary remains visible',
      'No overlap',
      'Staged appears only after a Skill is staged',
    ],
  },
  {
    id: 'context-observation-file-return',
    story: 'experience-context--observation-file-return',
    route: '/tasks/tsk_000000000000000000000001',
    expected: [
      'src/lib.rs opens in Review Files',
      'Leaving Review restores Context and Chat position',
    ],
  },
  {
    id: 'context-webkit-360',
    story: 'experience-responsive-feature-surface--context-webkit-360',
    route: '/tasks/tsk_000000000000000000000001',
    expected: ['No horizontal overflow'],
  },
  {
    id: 'context-zoom-200',
    story: 'experience-responsive-feature-surface--context-zoom-200',
    route: '/tasks/tsk_000000000000000000000001',
    expected: ['All controls operable'],
  },
  {
    id: 'context-forced-colors',
    story: 'experience-responsive-feature-surface--context-forced-colors',
    route: '/tasks/tsk_000000000000000000000001',
    expected: ['Focus and boundaries visible'],
  },
] as const;
