export class DesktopError extends Error {
  constructor(public readonly code: string, message: string) { super(message); this.name = 'DesktopError'; }
}
export function normalizeError(error: unknown): DesktopError {
  if (error instanceof DesktopError) return error;
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error && typeof error.code === 'string' && typeof error.message === 'string') {
    return new DesktopError(error.code, error.message);
  }
  return new DesktopError('unexpected', 'Something went wrong. Retry the action or restart JevCode.');
}
