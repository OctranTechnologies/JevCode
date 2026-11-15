import type { SkillSummary } from '../types/domain';

/**
 * Keeps the skills whose name or description contains every word of the query, ignoring case.
 * A blank query keeps all of them.
 */
export function filterSkills(skills: SkillSummary[], query: string): SkillSummary[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return skills;
  return skills.filter(skill => {
    const text = `${skill.name} ${skill.description}`.toLowerCase();
    return words.every(word => text.includes(word));
  });
}
