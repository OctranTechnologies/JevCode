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
  'linux-x86_64-deb',
  'linux-x86_64-rpm',
];

const preferredTargets = {
  'windows-x86_64': 'windows-x86_64-msi',
  'darwin-aarch64': 'darwin-aarch64-app',
  'darwin-x86_64': 'darwin-x86_64-app',
  'linux-x86_64': 'linux-x86_64-appimage',
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
    errors.push(`Manifest must contain exactly these platform and installer targets: ${expectedTargets.join(', ')}.`);
  }

  const expectedApiPath = `/repos/${repository}/releases/assets/`;
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
      || artifactUrl.search || artifactUrl.hash || !artifactUrl.pathname.startsWith(expectedApiPath)) {
      errors.push(`Manifest artifact URL must be an HTTPS GitHub API asset URL for ${target}.`);
      continue;
    }

    if (!assets.some(asset => asset?.url === entry.url)) {
      errors.push(`Manifest artifact for ${target} is not attached to this GitHub Release.`);
    }
  }

  for (const [baseTarget, preferredTarget] of Object.entries(preferredTargets)) {
    const base = manifest.platforms[baseTarget];
    const preferred = manifest.platforms[preferredTarget];
    if (base && preferred && (base.url !== preferred.url || base.signature !== preferred.signature)) {
      errors.push(`Generic target ${baseTarget} must match its preferred installer target ${preferredTarget}.`);
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
  return JSON.parse(readFileSync(path, 'utf8'));
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

  const manifest = readJson(manifestPath);
  const assets = readJson(assetsPath);
  const errors = validateUpdaterRelease({ manifest, assets, repository, tag });
  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exit(1);
  }
  console.log(`Validated signed updater manifest for ${tag} (${requiredUpdaterTargets.join(', ')}).`);
}
