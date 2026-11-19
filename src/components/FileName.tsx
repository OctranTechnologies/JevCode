import { splitFilePath } from '../lib/paths';

/** A file path that shrinks the folder first, then the base name, and never the extension. */
export function FileName({ path, directory = false }: { path: string; directory?: boolean }) {
  const { dir, stem, ext } = splitFilePath(path, directory);
  return <span className="file-label" title={path}>
    {dir && <span className="file-label-dir">{dir}</span>}
    <span className="file-label-stem">{stem}</span>
    {ext && <span className="file-label-ext">{ext}</span>}
  </span>;
}
