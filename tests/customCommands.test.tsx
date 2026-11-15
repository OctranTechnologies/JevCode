import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { CommandPreview, CustomCommandRow } from '../src/features/workspaces/CustomCommandsPanel';
import type { CustomCommand, CustomCommandPreview } from '../src/types/domain';

const ready: CustomCommand = { id: '.claude/commands/review.md', format: 'claude_command', name: 'review', description: 'Review the diff.', blocked: null, warnings: [] };
const blocked: CustomCommand = { id: '.gemini/commands/deploy.toml', format: 'gemini_command', name: 'deploy', description: null, blocked: 'Gemini CLI commands are TOML files.', warnings: [] };
const plain: CustomCommandPreview = { command: ready, target: '.agents/skills/review/SKILL.md', skillText: '---\nname: review\n---\n' };
const withShell: CustomCommandPreview = { ...plain, command: { ...ready, warnings: ['Shell commands written as !`...` do not run from a skill.'] } };
const noop = () => {};
const controls = { busy: false, onAcknowledge: noop, onCreate: noop, onClose: noop };

/** The markup of the button whose label matches, so disabled state can be checked on its own. */
function button(html: string, label: string) {
  return html.split('<button').find(part => part.includes(`${label}</`)) ?? '';
}

describe('custom command rows', () => {
  it('offers a preview only for commands that can be converted', () => {
    const html = renderToStaticMarkup(<CustomCommandRow item={ready} busy={false} previewOpen={false} onPreview={noop} />);
    expect(html).toContain('Preview conversion');
    expect(html).toContain('Ready');

    const skipped = renderToStaticMarkup(<CustomCommandRow item={blocked} busy={false} previewOpen={false} onPreview={noop} />);
    expect(skipped).toContain('Not converted');
    expect(skipped).toContain('TOML files');
    expect(skipped).not.toContain('Preview conversion');
  });

  it('counts the behavior changes a conversion would need confirmed', () => {
    const html = renderToStaticMarkup(<CustomCommandRow item={withShell.command} busy={false} previewOpen={false} onPreview={noop} />);
    expect(html).toContain('1 behavior change to review');
  });
});

describe('custom command preview', () => {
  it('keeps creation disabled until a behavior change is acknowledged', () => {
    const html = renderToStaticMarkup(<CommandPreview view={withShell} acknowledged={false} {...controls} />);
    expect(html).toContain('A skill cannot keep these parts of the command');
    expect(button(html, 'Create skill')).toContain('disabled=""');

    const acknowledged = renderToStaticMarkup(<CommandPreview view={withShell} acknowledged {...controls} />);
    expect(button(acknowledged, 'Create skill')).not.toContain('disabled');
  });

  it('needs no acknowledgement when nothing would be lost', () => {
    const html = renderToStaticMarkup(<CommandPreview view={plain} acknowledged={false} {...controls} />);
    expect(html).not.toContain('I understand these changes');
    expect(button(html, 'Create skill')).not.toContain('disabled');
  });

  it('shows the target and the exact text the skill would contain', () => {
    const html = renderToStaticMarkup(<CommandPreview view={plain} acknowledged={false} {...controls} />);
    expect(html).toContain('<code>.agents/skills/review/SKILL.md</code>');
    expect(html).toContain('name: review');
  });
});
