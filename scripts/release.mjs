import { spawnSync } from 'node:child_process';
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const VERSION_PATHS = [
  'src-tauri/tauri.conf.json',
  'package.json',
  'package-lock.json',
  'src-tauri/Cargo.toml',
  'src-tauri/Cargo.lock',
];

export function parseStableVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(version);
  if (!match) throw new Error(`Expected a stable semantic version, received "${version}".`);

  const [major, minor, patch] = match.slice(1).map(Number);
  if (![major, minor, patch].every(Number.isSafeInteger)) {
    throw new Error(`Version components must be safe integers: "${version}".`);
  }
  return { major, minor, patch };
}

export function bumpVersion(version, releaseType) {
  const current = parseStableVersion(version);
  let nextVersion;
  if (releaseType === 'patch') nextVersion = `${current.major}.${current.minor}.${current.patch + 1}`;
  else if (releaseType === 'minor') nextVersion = `${current.major}.${current.minor + 1}.0`;
  else if (releaseType === 'major') nextVersion = `${current.major + 1}.0.0`;
  else throw new Error(`Unknown release type "${releaseType}". Use patch, minor, or major.`);
  parseStableVersion(nextVersion);
  return nextVersion;
}

function parseJson(contents, filename) {
  try {
    return JSON.parse(contents);
  } catch (error) {
    throw new Error(`${filename} is not valid JSON: ${error.message}`);
  }
}

function getCargoPackageVersion(contents) {
  const packageStart = contents.indexOf('[package]');
  if (packageStart < 0) throw new Error('src-tauri/Cargo.toml has no [package] section.');
  const afterPackage = packageStart + '[package]'.length;
  const nextSectionOffset = contents.slice(afterPackage).search(/^\[/m);
  const packageSection = contents.slice(
    afterPackage,
    nextSectionOffset < 0 ? undefined : afterPackage + nextSectionOffset,
  );
  const version = /^version\s*=\s*"([^"]+)"\s*$/m.exec(packageSection)?.[1];
  if (!version) throw new Error('src-tauri/Cargo.toml [package] has no version field.');
  return version;
}

function getCargoLockVersion(contents) {
  const version = /^\[\[package\]\]\r?\nname\s*=\s*"jevcode"\r?\nversion\s*=\s*"([^"]+)"/m.exec(contents)?.[1];
  if (!version) throw new Error('src-tauri/Cargo.lock has no jevcode package entry.');
  return version;
}

export function readVersionSet(contents) {
  const tauriConfig = parseJson(contents.tauriConfig, 'src-tauri/tauri.conf.json');
  const packageJson = parseJson(contents.packageJson, 'package.json');
  const packageLock = parseJson(contents.packageLock, 'package-lock.json');
  const cargoLockVersion = getCargoLockVersion(contents.cargoLock);

  return {
    tauriConfig: tauriConfig.version,
    packageJson: packageJson.version,
    packageLock: packageLock.version,
    packageLockRoot: packageLock.packages?.['']?.version,
    cargoToml: getCargoPackageVersion(contents.cargoToml),
    cargoLock: cargoLockVersion,
  };
}

export function verifyVersionSet(contents, expectedVersion) {
  const versions = readVersionSet(contents);
  const uniqueVersions = [...new Set(Object.values(versions))];
  if (uniqueVersions.length !== 1 || uniqueVersions[0] === undefined) {
    const detail = Object.entries(versions).map(([file, value]) => `${file}=${value ?? '<missing>'}`).join(', ');
    throw new Error(`Application versions are inconsistent: ${detail}.`);
  }

  const version = uniqueVersions[0];
  parseStableVersion(version);
  if (expectedVersion && version !== expectedVersion) {
    throw new Error(`Release tag version ${expectedVersion} does not match application version ${version}.`);
  }
  return version;
}

function replaceRootJsonVersion(contents, filename, version) {
  const parsed = parseJson(contents, filename);
  if (typeof parsed.version !== 'string') throw new Error(`${filename} has no root version field.`);
  let replaced = false;
  const updated = contents.replace(/^( {2}"version"\s*:\s*")[^"]+("\s*,?)$/m, (_line, before, after) => {
    replaced = true;
    return `${before}${version}${after}`;
  });
  if (!replaced) throw new Error(`Could not safely locate the root version field in ${filename}.`);
  return updated;
}

function replaceCargoTomlVersion(contents, version) {
  const packageStart = contents.indexOf('[package]');
  const afterPackage = packageStart + '[package]'.length;
  const nextSectionOffset = contents.slice(afterPackage).search(/^\[/m);
  const sectionEnd = nextSectionOffset < 0 ? contents.length : afterPackage + nextSectionOffset;
  const section = contents.slice(packageStart, sectionEnd);
  const updatedSection = section.replace(/^(version\s*=\s*")[^"]+("\s*)$/m, (_line, before, after) => `${before}${version}${after}`);
  if (updatedSection === section) throw new Error('Could not safely update the [package] version in src-tauri/Cargo.toml.');
  return contents.slice(0, packageStart) + updatedSection + contents.slice(sectionEnd);
}

function replaceCargoLockVersion(contents, version) {
  const pattern = /^(\[\[package\]\]\r?\nname\s*=\s*"jevcode"\r?\nversion\s*=\s*")[^"]+("\s*)$/m;
  const updated = contents.replace(pattern, (_line, before, after) => `${before}${version}${after}`);
  if (updated === contents) throw new Error('Could not safely update jevcode in src-tauri/Cargo.lock.');
  return updated;
}

export function synchronizeVersionContents(contents, version) {
  parseStableVersion(version);
  const packageLock = parseJson(contents.packageLock, 'package-lock.json');
  if (!packageLock.packages?.['']) {
    throw new Error('package-lock.json is missing its root package entry. Run npm install to regenerate it.');
  }

  const updated = {
    tauriConfig: replaceRootJsonVersion(contents.tauriConfig, 'src-tauri/tauri.conf.json', version),
    packageJson: replaceRootJsonVersion(contents.packageJson, 'package.json', version),
    packageLock: '',
    cargoToml: replaceCargoTomlVersion(contents.cargoToml, version),
    cargoLock: replaceCargoLockVersion(contents.cargoLock, version),
  };
  packageLock.version = version;
  packageLock.packages[''].version = version;
  const newline = contents.packageLock.includes('\r\n') ? '\r\n' : '\n';
  updated.packageLock = `${JSON.stringify(packageLock, null, 2).replace(/\n/g, newline)}${newline}`;
  verifyVersionSet(updated, version);
  return updated;
}

function readVersionFiles(root = ROOT) {
  return {
    tauriConfig: readFileSync(path.join(root, VERSION_PATHS[0]), 'utf8'),
    packageJson: readFileSync(path.join(root, VERSION_PATHS[1]), 'utf8'),
    packageLock: readFileSync(path.join(root, VERSION_PATHS[2]), 'utf8'),
    cargoToml: readFileSync(path.join(root, VERSION_PATHS[3]), 'utf8'),
    cargoLock: readFileSync(path.join(root, VERSION_PATHS[4]), 'utf8'),
  };
}

function writeVersionFiles(contents, root = ROOT) {
  const values = [contents.tauriConfig, contents.packageJson, contents.packageLock, contents.cargoToml, contents.cargoLock];
  VERSION_PATHS.forEach((relativePath, index) => writeFileSync(path.join(root, relativePath), values[index]));
}

function run(command, args, { allowFailure = false, inherit = false } = {}) {
  const result = spawnSync(command, args, {
    cwd: ROOT,
    encoding: 'utf8',
    stdio: inherit ? 'inherit' : 'pipe',
    windowsHide: true,
    shell: process.platform === 'win32' && command === 'npm',
  });
  if (result.error) throw result.error;
  if (!allowFailure && result.status !== 0) {
    const detail = result.stderr?.trim() || result.stdout?.trim() || `${command} exited with ${result.status}`;
    throw new Error(detail);
  }
  return { status: result.status ?? 1, stdout: result.stdout ?? '', stderr: result.stderr ?? '' };
}

function requireCleanMainBranch() {
  const status = run('git', ['status', '--porcelain=v1', '--untracked-files=all']).stdout.trim();
  if (status) throw new Error('Release requires a clean working tree. Commit or stash all changes first.');

  const branch = run('git', ['branch', '--show-current']).stdout.trim();
  if (branch !== 'main') throw new Error(`Release must run from main; current branch is "${branch || 'detached HEAD'}".`);
  if (!run('git', ['remote', 'get-url', 'origin'], { allowFailure: true }).stdout.trim()) {
    throw new Error('Git remote "origin" is not configured.');
  }
}

function ensureOriginIsCurrent() {
  run('git', ['fetch', 'origin', 'main', '--quiet']);
  const local = run('git', ['rev-parse', 'HEAD']).stdout.trim();
  const remote = run('git', ['rev-parse', 'refs/remotes/origin/main']).stdout.trim();
  if (local !== remote) throw new Error('Local main must match origin/main before releasing. Pull or push main, then retry.');
}

function ensureTagIsAvailable(tag) {
  const localTag = run('git', ['show-ref', '--verify', '--quiet', `refs/tags/${tag}`], { allowFailure: true });
  if (localTag.status === 0) throw new Error(`Tag ${tag} already exists locally.`);
  const remoteTag = run('git', ['ls-remote', '--tags', 'origin', `refs/tags/${tag}`]);
  if (remoteTag.stdout.trim()) throw new Error(`Tag ${tag} already exists on origin.`);
}

function assertOnlyVersionFilesChanged() {
  const changed = run('git', ['status', '--porcelain=v1', '--untracked-files=all']).stdout
    .split(/\r?\n/)
    .filter(Boolean)
    .map((line) => line.slice(3).replaceAll('\\', '/'));
  const expected = new Set(VERSION_PATHS);
  const unexpected = changed.filter((file) => !expected.has(file));
  if (unexpected.length) throw new Error(`Checks changed unexpected files: ${unexpected.join(', ')}.`);
}

function verifyCommand(expectedTag) {
  const contents = readVersionFiles();
  const expectedVersion = expectedTag?.replace(/^v/, '');
  const version = verifyVersionSet(contents, expectedVersion);
  console.log(`Application versions are synchronized at ${version}.`);
}

function releaseCommand(releaseType, dryRun) {
  requireCleanMainBranch();
  const original = readVersionFiles();
  const currentVersion = verifyVersionSet(original);
  const nextVersion = bumpVersion(currentVersion, releaseType);
  const tag = `v${nextVersion}`;

  if (dryRun) {
    console.log(`Dry run: ${currentVersion} → ${nextVersion} (${tag}).`);
    console.log('Would verify origin/main and tag availability, synchronize all five version files, run npm run check, commit the bump, then push main and the tag.');
    return;
  }

  ensureOriginIsCurrent();
  ensureTagIsAvailable(tag);
  const updated = synchronizeVersionContents(original, nextVersion);
  writeVersionFiles(updated);

  let committed = false;
  try {
    console.log(`Validating JevCode ${nextVersion} before creating the release tag...`);
    run('npm', ['run', 'check'], { inherit: true });
    assertOnlyVersionFilesChanged();
    run('git', ['add', '--', ...VERSION_PATHS]);
    run('git', ['commit', '-m', `chore(release): ${tag}`], { inherit: true });
    committed = true;
  } catch (error) {
    if (!committed) {
      run('git', ['reset', '--', ...VERSION_PATHS], { allowFailure: true });
      writeVersionFiles(original);
      throw new Error(`${error.message}\nThe version files were restored; no release tag was pushed.`);
    }
    throw error;
  }

  run('git', ['tag', '--annotate', tag, '--message', `JevCode ${tag}`]);
  run('git', ['push', 'origin', 'main'], { inherit: true });
  try {
    run('git', ['push', 'origin', tag], { inherit: true });
  } catch (error) {
    throw new Error(`${error.message}\nThe version commit is on origin/main, but ${tag} is still local. Run: git push origin ${tag}`);
  }
  console.log(`Pushed main and ${tag}; GitHub Actions will build and publish the release.`);
}

function main(args) {
  if (args[0] === 'verify') {
    const expectedTag = args[1];
    verifyCommand(expectedTag);
    return;
  }

  const dryRun = args.includes('--dry-run');
  const releaseType = args.find((arg) => !arg.startsWith('--'));
  if (!releaseType) throw new Error('Choose patch, minor, or major.');
  releaseCommand(releaseType, dryRun);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`Release stopped: ${error.message}`);
    process.exitCode = 1;
  }
}
