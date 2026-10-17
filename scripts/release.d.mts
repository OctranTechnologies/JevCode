export interface VersionFileContents {
  tauriConfig: string;
  packageJson: string;
  packageLock: string;
  cargoToml: string;
  cargoLock: string;
}

export function parseStableVersion(version: string): { major: number; minor: number; patch: number };
export function bumpVersion(version: string, releaseType: 'patch' | 'minor' | 'major'): string;
export function readVersionSet(contents: VersionFileContents): Record<string, string | undefined>;
export function verifyVersionSet(contents: VersionFileContents, expectedVersion?: string): string;
export function synchronizeVersionContents(contents: VersionFileContents, version: string): VersionFileContents;
