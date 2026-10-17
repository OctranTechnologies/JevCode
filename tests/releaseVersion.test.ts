import { describe, expect, it } from 'vitest';
import {
  bumpVersion,
  parseStableVersion,
  readVersionSet,
  synchronizeVersionContents,
  verifyVersionSet,
  type VersionFileContents,
} from '../scripts/release.mjs';

function versionFiles(version = '0.1.0'): VersionFileContents {
  return {
    tauriConfig: `{
  "$schema": "https://schema.tauri.app/config/2",
  "version": "${version}",
  "productName": "JevCode"
}\n`,
    packageJson: `{
  "name": "jevcode",
  "version": "${version}"
}\n`,
    packageLock: `${JSON.stringify({
      name: 'jevcode',
      version,
      lockfileVersion: 3,
      packages: { '': { name: 'jevcode', version } },
    }, null, 2)}\n`,
    cargoToml: `[package]\nname = "jevcode"\nversion = "${version}"\nedition = "2021"\n`,
    cargoLock: `version = 4\n\n[[package]]\nname = "jevcode"\nversion = "${version}"\ndependencies = []\n`,
  };
}

describe('release version workflow', () => {
  it('bumps stable semantic versions predictably', () => {
    expect(bumpVersion('0.1.9', 'patch')).toBe('0.1.10');
    expect(bumpVersion('0.1.9', 'minor')).toBe('0.2.0');
    expect(bumpVersion('0.1.9', 'major')).toBe('1.0.0');
    expect(() => parseStableVersion('1.2.3-beta.1')).toThrow('stable semantic version');
  });

  it('detects mismatched Tauri, npm, and Cargo versions', () => {
    const contents = versionFiles();
    expect(verifyVersionSet(contents)).toBe('0.1.0');
    expect(verifyVersionSet(contents, '0.1.0')).toBe('0.1.0');
    expect(() => verifyVersionSet(contents, '0.2.0')).toThrow('does not match');

    const inconsistent = { ...contents, cargoToml: contents.cargoToml.replace('0.1.0', '0.2.0') };
    expect(() => verifyVersionSet(inconsistent)).toThrow('Application versions are inconsistent');
  });

  it('synchronizes app manifests and lockfiles from the Tauri release version', () => {
    const updated = synchronizeVersionContents(versionFiles(), '0.2.0');

    expect(readVersionSet(updated)).toEqual({
      tauriConfig: '0.2.0',
      packageJson: '0.2.0',
      packageLock: '0.2.0',
      packageLockRoot: '0.2.0',
      cargoToml: '0.2.0',
      cargoLock: '0.2.0',
    });
    expect(verifyVersionSet(updated, '0.2.0')).toBe('0.2.0');
  });
});
