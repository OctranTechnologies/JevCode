import type { ReactNode } from 'react';

export function ReleaseNotes({ version, body, publishedAt, className = '' }: {
  version: string;
  body: string | null;
  publishedAt?: string | null;
  className?: string;
}) {
  const notes = body?.trim();
  return <section className={`release-notes ${className}`.trim()} aria-label={`What's New in v${version}`}>
    <header className="release-notes-heading">
      <h3>What's New in v{version}</h3>
      {publishedAt && <time dateTime={publishedAt}>{formatReleaseDate(publishedAt)}</time>}
    </header>
    <div className="release-notes-body">
      {notes ? renderMarkdown(notes) : <p>No release notes were provided.</p>}
    </div>
  </section>;
}

function renderMarkdown(markdown: string): ReactNode[] {
  const lines = markdown.replace(/\r\n?/g, '\n').split('\n');
  const blocks: ReactNode[] = [];
  let index = 0;

  while (index < lines.length) {
    const line = lines[index].trim();
    if (!line) {
      index++;
      continue;
    }

    const heading = /^(#{1,4})\s+(.+)$/.exec(line);
    if (heading) {
      blocks.push(<h4 key={`heading-${index}`}>{renderInline(heading[2])}</h4>);
      index++;
      continue;
    }

    const bullet = /^[-*+]\s+(.+)$/.exec(line);
    if (bullet) {
      const items: ReactNode[] = [];
      while (index < lines.length) {
        const item = /^\s*[-*+]\s+(.+)$/.exec(lines[index]);
        if (!item) break;
        items.push(<li key={`item-${index}`}>{renderInline(item[1])}</li>);
        index++;
      }
      blocks.push(<ul key={`list-${index}`}>{items}</ul>);
      continue;
    }

    const ordered = /^\d+[.)]\s+(.+)$/.exec(line);
    if (ordered) {
      const items: ReactNode[] = [];
      while (index < lines.length) {
        const item = /^\s*\d+[.)]\s+(.+)$/.exec(lines[index]);
        if (!item) break;
        items.push(<li key={`item-${index}`}>{renderInline(item[1])}</li>);
        index++;
      }
      blocks.push(<ol key={`list-${index}`}>{items}</ol>);
      continue;
    }

    const paragraph: string[] = [];
    while (index < lines.length && lines[index].trim()) {
      if (/^\s*(?:#{1,4}\s+|[-*+]\s+|\d+[.)]\s+)/.test(lines[index])) break;
      paragraph.push(lines[index].trim());
      index++;
    }
    if (paragraph.length) blocks.push(<p key={`paragraph-${index}`}>{renderInline(paragraph.join(' '))}</p>);
  }

  return blocks;
}

function renderInline(text: string): ReactNode[] {
  const pattern = /(\[([^\]]+)\]\(([^)]+)\)|\*\*([^*]+)\*\*|__([^_]+)__|~~([^~]+)~~|`([^`]+)`|\*([^*\n]+)\*|_([^_\n]+)_)/g;
  const nodes: ReactNode[] = [];
  let cursor = 0;
  let index = 0;
  let match: RegExpExecArray | null;

  while ((match = pattern.exec(text)) !== null) {
    if (match.index > cursor) nodes.push(text.slice(cursor, match.index));
    if (match[2]) nodes.push(<span className="release-notes-link-text" key={`inline-${index}`} title={match[3]}>{match[2]}</span>);
    else if (match[4] || match[5]) nodes.push(<strong key={`inline-${index}`}>{match[4] ?? match[5]}</strong>);
    else if (match[6]) nodes.push(<del key={`inline-${index}`}>{match[6]}</del>);
    else if (match[7]) nodes.push(<code key={`inline-${index}`}>{match[7]}</code>);
    else if (match[8] || match[9]) nodes.push(<em key={`inline-${index}`}>{match[8] ?? match[9]}</em>);
    cursor = pattern.lastIndex;
    index++;
  }
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}

function formatReleaseDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(date);
}
