import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import console from 'node:console';
import process from 'node:process';

const stableVersionPattern = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;

function parseStableVersion(value) {
  const match = typeof value === 'string' ? stableVersionPattern.exec(value) : null;
  return match ? match.slice(1).map(part => BigInt(part)) : null;
}

export function compareStableVersions(left, right) {
  const a = parseStableVersion(left);
  const b = parseStableVersion(right);
  if (!a || !b) throw new TypeError('Both versions must be stable semantic versions.');
  for (let index = 0; index < 3; index += 1) {
    if (a[index] < b[index]) return -1;
    if (a[index] > b[index]) return 1;
  }
  return 0;
}

export function validateReleaseOrder({ releases, tag }) {
  const errors = [];
  if (!parseStableVersion(tag)) {
    return ['Release tag must be a stable semantic version such as v0.1.0.'];
  }
  if (!Array.isArray(releases)) {
    return ['GitHub Releases response is not a list.'];
  }

  const stableReleases = releases
    .filter(release => release && release.draft === false && release.prerelease === false && parseStableVersion(release.tag_name))
    .sort((left, right) => compareStableVersions(left.tag_name, right.tag_name));
  const latest = stableReleases.at(-1)?.tag_name;
  if (latest && compareStableVersions(tag, latest) <= 0) {
    errors.push(`Release tag ${tag} must be newer than the latest published stable release ${latest}.`);
  }
  return errors;
}

function getArgument(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const releasesPath = getArgument('--releases');
  const tag = getArgument('--tag');
  if (!releasesPath || !tag) {
    console.error('Usage: node scripts/validate-release-order.mjs --releases releases.json --tag v0.1.0');
    process.exit(2);
  }

  const releases = JSON.parse(readFileSync(releasesPath, 'utf8'));
  const errors = validateReleaseOrder({ releases, tag });
  if (errors.length) {
    for (const error of errors) console.error(error);
    process.exit(1);
  }
  console.log(`Release tag ${tag} is newer than every published stable release.`);
}
