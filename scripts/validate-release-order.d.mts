export function compareStableVersions(left: string, right: string): number;

export function validateReleaseOrder(input: {
  releases: unknown;
  tag: string;
}): string[];
