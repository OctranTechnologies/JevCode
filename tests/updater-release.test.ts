import { readFileSync } from 'node:fs';
import { Buffer } from 'node:buffer';
import { URL } from 'node:url';
import { describe, expect, it } from 'vitest';
import { requiredUpdaterTargets, validateUpdaterRelease } from '../scripts/validate-updater-release.mjs';

const repository = 'OctranTechnologies/JevCode';
const tag = 'v0.1.1';
const apiUrl = `https://api.github.com/repos/${repository}/releases/assets`;

function makeRelease() {
  const platforms = Object.fromEntries(requiredUpdaterTargets.map((target, index) => [target, {
    url: `${apiUrl}/${index + 100}`,
    signature: `signature-${target}-${'x'.repeat(48)}`,
  }]));
  for (const [baseTarget, preferredTarget] of Object.entries({
    'windows-x86_64': 'windows-x86_64-msi',
    'darwin-aarch64': 'darwin-aarch64-app',
    'darwin-x86_64': 'darwin-x86_64-app',
    'linux-x86_64': 'linux-x86_64-appimage',
  })) {
    platforms[baseTarget] = platforms[preferredTarget];
  }
  const assets = Object.values(platforms).map(({ url }) => ({ url }));
  return {
    manifest: { version: '0.1.1', notes: 'Test release', pub_date: '2026-10-17T00:00:00Z', platforms },
    assets,
    repository,
    tag,
  };
}

describe('updater release manifest validation', () => {
  it('accepts a complete HTTPS manifest whose platform files belong to the tagged release', () => {
    expect(validateUpdaterRelease(makeRelease())).toEqual([]);
  });

  it('rejects an incomplete platform set', () => {
    const release = makeRelease();
    delete release.manifest.platforms['darwin-aarch64'];

    expect(validateUpdaterRelease(release).join(' ')).toContain('exactly these platform and installer targets');
  });

  it('rejects HTTP links and URLs that point to assets outside this release', () => {
    const release = makeRelease();
    release.manifest.platforms['windows-x86_64'].url = `http://api.github.com/repos/${repository}/releases/assets/999`;

    expect(validateUpdaterRelease(release).join(' ')).toContain('HTTPS GitHub API asset URL');
  });

  it('rejects a manifest URL that is not an attached release asset', () => {
    const release = makeRelease();
    release.manifest.platforms['linux-x86_64'].url = `${apiUrl}/999`;

    expect(validateUpdaterRelease(release).join(' ')).toContain('not attached to this GitHub Release');
  });

  it('rejects a generic platform mapping that points to the wrong installer', () => {
    const release = makeRelease();
    release.manifest.platforms['windows-x86_64'] = {
      url: `${apiUrl}/999`,
      signature: 'x'.repeat(64),
    };

    expect(validateUpdaterRelease(release).join(' ')).toContain('must match its preferred installer target');
  });

  it('rejects version mismatches, prerelease tags, and missing signatures', () => {
    const release = makeRelease();
    release.manifest.platforms['windows-x86_64'].signature = '';

    expect(validateUpdaterRelease({ ...release, tag: 'v0.1.1-rc.1' }).join(' ')).toContain('stable semantic version');
    expect(validateUpdaterRelease({ ...release, manifest: { ...release.manifest, version: '0.1.0' } }).join(' ')).toContain('must match release tag');
    expect(validateUpdaterRelease(release).join(' ')).toContain('signature is missing');
  });

  it('keeps the app pinned to the HTTPS GitHub endpoint and configured signing key', () => {
    const config = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
    const updater = config.plugins.updater;
    const publicKey = Buffer.from(updater.pubkey, 'base64').toString('utf8');

    expect(config.bundle.createUpdaterArtifacts).toBe(true);
    expect(updater.endpoints).toEqual([`https://github.com/${repository}/releases/latest/download/latest.json`]);
    expect(updater.endpoints.every((endpoint: string) => new URL(endpoint).protocol === 'https:')).toBe(true);
    expect(updater.dangerousInsecureTransportProtocol).not.toBe(true);
    expect(updater.requireSignedVersion).toBe(true);
    expect(updater.allowDowngrades).toBe(false);
    expect(publicKey).toContain('minisign public key: 916CDEFC01AA297D');
  });
});
