import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { MarkdownMessage } from '../src/features/agent/MarkdownMessage';

describe('conversation Markdown', () => {
  it('renders inline emphasis, lists, fenced code, and GitHub-flavored tables', () => {
    const content = '- **hello.py** prints `Hello, world!`\n- second item\n\n```python\nprint("Hello, world!")\n```\n\n| Name | Kind |\n| --- | --- |\n| app.py | file |';
    const markup = renderToStaticMarkup(createElement(MarkdownMessage, { content }));

    expect(markup).toContain('<strong>hello.py</strong>');
    expect(markup).toContain('<code>Hello, world!</code>');
    expect(markup).toContain('<ul>');
    expect(markup).toContain('class="language-python"');
    expect(markup).toContain('<table>');
    expect(markup).toContain('<th>Name</th>');
  });

  it('does not render raw HTML from message content', () => {
    const content = '<img src=x onerror="alert(1)" />\n\nSafe **text**.';
    const markup = renderToStaticMarkup(createElement(MarkdownMessage, { content }));

    expect(markup).not.toContain('<img');
    expect(markup).toContain('&lt;img src=x onerror=&quot;alert(1)&quot; /&gt;');
    expect(markup).toContain('<strong>text</strong>');
  });
});
