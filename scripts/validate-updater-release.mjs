import { readFileSync } from 'node:fs';
import { pathToFileURL, URL } from 'node:url';
import console from 'node:console';
import process from 'node:process';

export const requiredUpdaterTargets = [
  'windows-x86_64',
  'darwin-aarch64',
  'darwin-x86_64',
  'linux-x86_64',
  'windows-x86_64-msi',
  'windows-x86_64-nsis',
  'darwin-aarch64-app',
  'darwin-x86_64-app',
  'linux-x86_64-appimage',
];

export function parseUpdaterManifest(json) {
  if (typeof json !== 'string') return { manifest: null, error: 'Updater manifest is not text.' };
  try {
    return { manifest: JSON.parse(json), error: null };
  } catch {
    return { manifest: null, error: 'Updater manifest is malformed JSON.' };
  }
}

const preferredTargets = {
  'windows-x86_64': 'windows-x86_64-msi',
  'darwin-aarch64': 'darwin-aarch64-app',
  'darwin-x86_64': 'darwin-x86_64-app',
  'linux-x86_64': 'linux-x86_64-appimage',
};

const targetAssetSuffix = {
  'windows-x86_64-msi': '.msi',
  'windows-x86_64-nsis': '-setup.exe',
  'darwin-aarch64-app': '.app.tar.gz',
  'darwin-x86_64-app': '.app.tar.gz',
  'linux-x86_64-appimage': '.appimage',
};

export function validateUpdaterRelease({ manifest, assets, repository, tag }) {
  const errors = [];
  const version = tag?.replace(/^v/, '');

  if (!/^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/.test(tag ?? '')) {
    errors.push('Release tag must be a stable semantic version such as v0.1.0.');
  }
  if (manifest?.version !== version) {
    errors.push(`Manifest version must match release tag ${tag}.`);
  }
  if (!manifest?.platforms || typeof manifest.platforms !== 'object' || Array.isArray(manifest.platforms)) {
    errors.push('Updater manifest has no platform map.');
    return errors;
  }
  if (!Array.isArray(assets)) {
    errors.push('GitHub Release assets are unavailable.');
    return errors;
  }

  const actualTargets = Object.keys(manifest.platforms).sort();
  const expectedTargets = [...requiredUpdaterTargets].sort();
  if (JSON.stringify(actualTargets) !== JSON.stringify(expectedTargets)) {
    errors.push(`Manifest must contain exactly these supported platform and updater targets: ${expectedTargets.join(', ')}.`);
  }

  if (typeof repository !== 'string' || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    errors.push('GitHub repository must use the owner/repository format.');
    return errors;
  }
  const expectedApiPath = `/repos/${repository}/releases/assets/`;
  const expectedApiPathPattern = expectedApiPath.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  for (const target of requiredUpdaterTargets) {
    const entry = manifest.platforms[target];
    if (!entry || typeof entry !== 'object') {
      errors.push(`Manifest is missing ${target}.`);
      continue;
    }

    if (typeof entry.signature !== 'string' || entry.signature.trim().length < 40) {
      errors.push(`Manifest signature is missing or malformed for ${target}.`);
    }

    let artifactUrl;
    try {
      artifactUrl = new URL(entry.url);
    } catch {
      errors.push(`Manifest artifact URL is invalid for ${target}.`);
      continue;
    }

    if (artifactUrl.protocol !== 'https:' || artifactUrl.hostname !== 'api.github.com'
      || artifactUrl.port || artifactUrl.username || artifactUrl.password
      || artifactUrl.search || artifactUrl.hash || !new RegExp(`^${expectedApiPathPattern}\\d+$`).test(artifactUrl.pathname)) {
      errors.push(`Manifest artifact URL must be an HTTPS GitHub API asset URL for ${target}.`);
      continue;
    }

    const asset = assets.find(candidate => candidate?.url === entry.url);
    if (!asset) {
      errors.push(`Manifest artifact for ${target} is not attached to this GitHub Release.`);
      continue;
    }

    const expectedTarget = preferredTargets[target] ?? target;
    const expectedSuffix = targetAssetSuffix[expectedTarget];
    const hasUnsafeAssetName = typeof asset.name === 'string'
      && [...asset.name].some(character => {
        const code = character.charCodeAt(0);
        return character === '/' || character === '\\' || code < 0x20 || code === 0x7f;
      });
    if (!expectedSuffix || typeof asset.name !== 'string'
      || hasUnsafeAssetName
      || !Number.isSafeInteger(asset.size) || asset.size <= 0
      || !asset.name.toLowerCase().endsWith(expectedSuffix.toLowerCase())) {
      errors.push(`Manifest artifact for ${target} does not match the expected ${expectedTarget} updater bundle.`);
    }

    const signatureAsset = assets.find(candidate => candidate?.name === `${asset.name}.sig`);
    if (!signatureAsset || !Number.isSafeInteger(signatureAsset.size) || signatureAsset.size <= 0
      || typeof signatureAsset.url !== 'string'
      || !new RegExp(`^https://api\\.github\\.com${expectedApiPathPattern}\\d+$`).test(signatureAsset.url)) {
      errors.push(`Uploaded updater signature asset is missing for ${target}.`);
    }
  }

  for (const [baseTarget, preferredTarget] of Object.entries(preferredTargets)) {
    const base = manifest.platforms[baseTarget];
    const preferred = manifest.platforms[preferredTarget];
    if (base && preferred && (base.url !== preferred.url || base.signature !== preferred.signature)) {
      errors.push(`Generic target ${baseTarget} must match its preferred updater target ${preferredTarget}.`);
    }
  }

  if (manifest.notes != null && typeof manifest.notes !== 'string') {
    errors.push('Manifest notes must be text when present.');
  }
  if (manifest.pub_date != null && (typeof manifest.pub_date !== 'string' || Number.isNaN(Date.parse(manifest.pub_date)))) {
    errors.push('Manifest publish date is invalid when present.');
  }

  return errors;
}

function readJson(path) {
  const parsed = parseUpdaterManifest(readFileSync(path, 'utf8'));
  if (parsed.error) throw new Error(parsed.error);
  return parsed.manifest;
}

function getArgument(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const manifestPath = getArgument('--manifest');
  const assetsPath = getArgument('--assets');
  const repository = getArgument('--repository');
  const tag = getArgument('--tag');
  if (!manifestPath || !assetsPath || !repository || !tag) {
    console.error('Usage: node scripts/validate-updater-release.mjs --manifest latest.json --assets release-assets.json --repository owner/repo --tag v0.1.0');
    process.exit(2);
  }

  try {
    const manifest = readJson(manifestPath);
    const assets = readJson(assetsPath);
    const errors = validateUpdaterRelease({ manifest, assets, repository, tag });
    if (errors.length) {
      for (const error of errors) console.error(error);
      process.exit(1);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : 'Updater release validation failed.');
    process.exit(1);
  }
  console.log(`Validated signed updater manifest for ${tag} (${requiredUpdaterTargets.join(', ')}).`);
}
