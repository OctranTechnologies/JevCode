export class DesktopError extends Error {
  constructor(public readonly code: string, message: string) { super(message); this.name = 'DesktopError'; }
}
/** The one user-facing wording for a spawn blocked by unavailable containment. */
export const CONTAINMENT_UNAVAILABLE_MESSAGE = 'Command execution is unavailable: JevCode cannot enforce this task’s containment profile on this system. File edits still work.';
function withContainmentCopy(error: DesktopError): DesktopError {
  return error.code === 'containment_unavailable' && error.message !== CONTAINMENT_UNAVAILABLE_MESSAGE
    ? new DesktopError(error.code, CONTAINMENT_UNAVAILABLE_MESSAGE)
    : error;
}
export function normalizeError(error: unknown): DesktopError {
  if (error instanceof DesktopError) return withContainmentCopy(error);
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error && typeof error.code === 'string' && typeof error.message === 'string') {
    return withContainmentCopy(new DesktopError(error.code, error.message));
  }
  return new DesktopError('unexpected', 'Something went wrong. Retry the action or restart JevCode.');
}
