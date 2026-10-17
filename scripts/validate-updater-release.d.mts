export const requiredUpdaterTargets: string[];

export interface UpdaterReleaseManifest {
  version?: string;
  notes?: unknown;
  pub_date?: unknown;
  platforms?: Record<string, { url?: string; signature?: string } | undefined>;
}

export interface GithubReleaseAsset {
  url?: string;
}

export function validateUpdaterRelease(input: {
  manifest: UpdaterReleaseManifest;
  assets: GithubReleaseAsset[];
  repository: string;
  tag: string;
}): string[];
