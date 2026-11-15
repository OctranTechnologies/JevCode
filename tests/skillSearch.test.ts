import { describe, expect, it } from 'vitest';
import { filterSkills } from '../src/lib/skills';
import type { SkillSummary } from '../src/types/domain';

function skill(name: string, description: string): SkillSummary {
  return {
    id: `project:${name}`,
    name,
    description,
    scope: 'project',
    source: `.agents/skills/${name}`,
    version: null,
    fingerprint: 'fingerprint',
    bytes: 100,
    allowedTools: [],
    valid: true,
    warnings: [],
  };
}

const skills = [
  skill('release-notes', 'Write release notes from merged pull requests'),
  skill('review', 'Review a diff for risky changes'),
  skill('docs-sync', 'Keep README pages in step with code'),
];

describe('filterSkills', () => {
  it('keeps every skill for a blank query', () => {
    expect(filterSkills(skills, '')).toEqual(skills);
    expect(filterSkills(skills, '   ')).toEqual(skills);
  });

  it('matches names and descriptions ignoring case', () => {
    expect(filterSkills(skills, 'RELEASE').map(item => item.name)).toEqual(['release-notes']);
    expect(filterSkills(skills, 'diff').map(item => item.name)).toEqual(['review']);
  });

  it('requires every word of the query to match', () => {
    expect(filterSkills(skills, 'release pull').map(item => item.name)).toEqual(['release-notes']);
    expect(filterSkills(skills, 'release diff')).toEqual([]);
  });
});
