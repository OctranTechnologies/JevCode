import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { FileName } from '../src/components/FileName';
import { splitFilePath } from '../src/lib/paths';

describe('splitFilePath', () => {
  it('separates the folder, base name and extension', () => {
    expect(splitFilePath('src/app/App.tsx')).toEqual({ dir: 'src/app/', stem: 'App', ext: '.tsx' });
  });

  it('treats dotfiles, extensionless names and very long suffixes as having no extension', () => {
    for (const path of ['.gitignore', 'Makefile', 'foo.', 'a.very-long-suffix']) {
      expect(splitFilePath(path).ext).toBe('');
    }
    expect(splitFilePath('v1.2', true)).toEqual({ dir: '', stem: 'v1.2', ext: '' });
  });

  it('keeps only the last extension of an archive name', () => {
    expect(splitFilePath('archive.tar.gz').ext).toBe('.gz');
  });

  it('splits Windows paths on the last separator', () => {
    expect(splitFilePath('C:\\Users\\me\\App.tsx')).toEqual({ dir: 'C:\\Users\\me\\', stem: 'App', ext: '.tsx' });
  });
});

describe('file name label', () => {
  it('renders the folder, base name and extension with the full path as a title', () => {
    const html = renderToStaticMarkup(<FileName path="src/app/App.tsx" />);

    expect(html).toContain('class="file-label-dir">src/app/</span>');
    expect(html).toContain('class="file-label-stem">App</span>');
    expect(html).toContain('class="file-label-ext">.tsx</span>');
    expect(html).toContain('title="src/app/App.tsx"');
  });

  it('omits the folder for a bare name and the extension for a dotfile', () => {
    const bare = renderToStaticMarkup(<FileName path="Makefile" />);
    expect(bare).not.toContain('file-label-dir');
    expect(bare).not.toContain('file-label-ext');

    const dotfile = renderToStaticMarkup(<FileName path=".gitignore" />);
    expect(dotfile).not.toContain('file-label-ext');
  });
});
