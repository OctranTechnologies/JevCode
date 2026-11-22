import type { Project } from '../../types/domain';

/** A task is uncontained when its project has opted into reduced safety. */
function projectUsesReducedSafety(project: Project | undefined): boolean {
  return project?.reducedSafetyOptIn === true;
}

export function UncontainedBadge({ project }: { project: Project | undefined }) {
  if (!projectUsesReducedSafety(project)) return null;
  return <span className="task-option-badge task-uncontained-badge" title="This task’s commands run without containment.">Uncontained</span>;
}
