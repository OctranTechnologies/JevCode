import { readFileSync } from 'node:fs';
import { Buffer } from 'node:buffer';
import { URL } from 'node:url';
import { describe, expect, it } from 'vitest';
import { parseUpdaterManifest, requiredUpdaterTargets, validateUpdaterRelease } from '../scripts/validate-updater-release.mjs';

const repository = 'OctranTechnologies/JevCode';
const tag = 'v0.1.1';
const apiUrl = `https://api.github.com/repos/${repository}/releases/assets`;

function makeRelease() {
  const platforms = Object.fromEntries(requiredUpdaterTargets.map((target, index) => [target, {
    url: `${apiUrl}/${index + 100}`,
    signature: `signature-${target}-${'x'.repeat(48)}`,
  }]));
  const preferredTargets: Record<string, string> = {
    'windows-x86_64': 'windows-x86_64-msi',
    'darwin-aarch64': 'darwin-aarch64-app',
    'darwin-x86_64': 'darwin-x86_64-app',
    'linux-x86_64': 'linux-x86_64-appimage',
  };
  for (const [baseTarget, preferredTarget] of Object.entries(preferredTargets)) {
    platforms[baseTarget] = platforms[preferredTarget];
  }
  const expectedNames: Record<string, string> = {
    'windows-x86_64-msi': 'JevCode_0.1.1_x64_en-US.msi',
    'windows-x86_64-nsis': 'JevCode_0.1.1_x64-setup.exe',
    'darwin-aarch64-app': 'JevCode_0.1.1_aarch64.app.tar.gz',
    'darwin-x86_64-app': 'JevCode_0.1.1_x64.app.tar.gz',
    'linux-x86_64-appimage': 'jevcode_0.1.1_amd64.AppImage',
    'linux-x86_64-deb': 'JevCode_0.1.1_amd64.deb',
    'linux-x86_64-rpm': 'JevCode-0.1.1-1.x86_64.rpm',
  };
  const targetForUrl = new Map<string, string>();
  for (const target of requiredUpdaterTargets) {
    targetForUrl.set(platforms[target].url, preferredTargets[target] ?? target);
  }
  const assets = [...targetForUrl].flatMap(([url, target], index) => {
    const name = expectedNames[target];
    return [
      { url, name, size: 1024 },
      { url: `${apiUrl}/${index + 500}`, name: `${name}.sig`, size: 128 },
    ];
  });
  return {
    manifest: { version: '0.1.1', notes: 'Test release', pub_date: '2026-10-17T00:00:00Z', platforms },
    assets,
    repository,
    tag,
  };
}

describe('updater release manifest validation', () => {
  it('rejects malformed latest.json safely', () => {
    expect(parseUpdaterManifest('{ "version": ')).toEqual({ manifest: null, error: 'Updater manifest is malformed JSON.' });
  });

  it('rejects a malformed repository ID before validating asset links', () => {
    const release = makeRelease();
    expect(validateUpdaterRelease({ ...release, repository: 'owner/repo?redirect=elsewhere' }).join(' ')).toContain('owner/repository format');
  });

  it('accepts a complete HTTPS manifest whose platform files belong to the tagged release', () => {
    expect(validateUpdaterRelease(makeRelease())).toEqual([]);
  });

  it('rejects an incomplete platform set', () => {
    const release = makeRelease();
    delete release.manifest.platforms['darwin-aarch64'];

    expect(validateUpdaterRelease(release).join(' ')).toContain('exactly these supported platform and updater targets');
  });

  it('rejects HTTP links and URLs that point to assets outside this release', () => {
    const release = makeRelease();
    release.manifest.platforms['windows-x86_64'].url = `http://api.github.com/repos/${repository}/releases/assets/999`;

    expect(validateUpdaterRelease(release).join(' ')).toContain('HTTPS GitHub API asset URL');
  });

  it('rejects noncanonical GitHub asset URLs and empty updater artifacts', () => {
    const portUrl = makeRelease();
    portUrl.manifest.platforms['linux-x86_64'].url = portUrl.manifest.platforms['linux-x86_64'].url.replace('api.github.com', 'api.github.com:8443');
    expect(validateUpdaterRelease(portUrl).join(' ')).toContain('HTTPS GitHub API asset URL');

    const emptyArtifact = makeRelease();
    const linuxUrl = emptyArtifact.manifest.platforms['linux-x86_64'].url;
    const artifact = emptyArtifact.assets.find(asset => asset.url === linuxUrl);
    if (artifact) artifact.size = 0;
    expect(validateUpdaterRelease(emptyArtifact).join(' ')).toContain('does not match the expected linux-x86_64-appimage updater bundle');
  });

  it('rejects updater asset names containing path separators or control characters', () => {
    const release = makeRelease();
    const linuxUrl = release.manifest.platforms['linux-x86_64-appimage'].url;
    const artifact = release.assets.find(asset => asset.url === linuxUrl);
    if (artifact) artifact.name = `../${artifact.name}${String.fromCharCode(0x1f)}`;

    expect(validateUpdaterRelease(release).join(' ')).toContain('does not match the expected linux-x86_64-appimage updater bundle');
  });

  it('rejects a manifest URL that is not an attached release asset', () => {
    const release = makeRelease();
    release.manifest.platforms['linux-x86_64'].url = `${apiUrl}/999`;

    expect(validateUpdaterRelease(release).join(' ')).toContain('not attached to this GitHub Release');
  });

  it('rejects a generic platform mapping that points to the wrong updater target', () => {
    const release = makeRelease();
    release.manifest.platforms['windows-x86_64'] = {
      url: `${apiUrl}/999`,
      signature: 'x'.repeat(64),
    };

    expect(validateUpdaterRelease(release).join(' ')).toContain('must match its preferred updater target');
  });

  it('requires the artifact format expected by each platform target', () => {
    const release = makeRelease();
    release.manifest.platforms['windows-x86_64-nsis'] = release.manifest.platforms['windows-x86_64-msi'];

    expect(validateUpdaterRelease(release).join(' ')).toContain('does not match the expected windows-x86_64-nsis updater bundle');
  });

  it('requires uploaded signatures for every platform-specific updater bundle', () => {
    const release = makeRelease();
    release.assets = release.assets.filter(asset => !asset.name.endsWith('.sig'));

    expect(validateUpdaterRelease(release).join(' ')).toContain('Uploaded updater signature asset is missing');

    const wrongRpmBundle = makeRelease();
    wrongRpmBundle.manifest.platforms['linux-x86_64-rpm'] = wrongRpmBundle.manifest.platforms['linux-x86_64-deb'];
    expect(validateUpdaterRelease(wrongRpmBundle).join(' ')).toContain('does not match the expected linux-x86_64-rpm updater bundle');
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
