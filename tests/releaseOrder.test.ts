import { describe, expect, it } from 'vitest';
import { compareStableVersions, validateReleaseOrder } from '../scripts/validate-release-order.mjs';

describe('release ordering', () => {
  it('compares numeric semantic version components', () => {
    expect(compareStableVersions('v0.10.0', 'v0.9.99')).toBeGreaterThan(0);
    expect(compareStableVersions('0.1.1', 'v0.1.1')).toBe(0);
  });

  it('accepts the next stable version and ignores drafts and prereleases', () => {
    const releases = [
      { tag_name: 'v0.1.0', draft: false, prerelease: false },
      { tag_name: 'v2.0.0', draft: true, prerelease: false },
      { tag_name: 'v1.0.0-rc.1', draft: false, prerelease: true },
    ];

    expect(validateReleaseOrder({ releases, tag: 'v0.1.1' })).toEqual([]);
  });

  it('rejects an older or repeated stable release before it can become latest', () => {
    const releases = [
      { tag_name: 'v0.1.1', draft: false, prerelease: false },
      { tag_name: 'v9.0.0', draft: true, prerelease: false },
      { tag_name: 'v10.0.0-beta.1', draft: false, prerelease: true },
    ];

    expect(validateReleaseOrder({ releases, tag: 'v0.1.0' }).join(' ')).toContain('newer than the latest published stable release v0.1.1');
    expect(validateReleaseOrder({ releases, tag: 'v0.1.1' }).join(' ')).toContain('newer than the latest published stable release v0.1.1');
  });

  it('rejects malformed tags and malformed release responses', () => {
    expect(validateReleaseOrder({ releases: [], tag: 'v0.1.0-rc.1' }).join(' ')).toContain('Release tag must be a stable semantic version');
    expect(validateReleaseOrder({ releases: {}, tag: 'v0.1.0' }).join(' ')).toContain('GitHub Releases response is not a list.');
  });
});
