export function displayPath(path: string) {
  if (path.startsWith('\\\\?\\UNC\\')) return `\\\\${path.slice(8)}`;
  if (path.startsWith('\\\\?\\')) return path.slice(4);
  return path;
}

/** Splits a path into folder, base name and extension. Dotfiles, directories and very long suffixes have no extension. */
export function splitFilePath(path: string, directory = false): { dir: string; stem: string; ext: string } {
  const slash = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  const dir = path.slice(0, slash + 1);
  const name = path.slice(slash + 1);
  const dot = name.lastIndexOf('.');
  if (directory || dot <= 0 || dot === name.length - 1 || name.length - dot > 12) return { dir, stem: name, ext: '' };
  return { dir, stem: name.slice(0, dot), ext: name.slice(dot) };
}
