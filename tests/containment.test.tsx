import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { ContainmentPanelView, ReducedSafetyDialogView } from '../src/features/workspaces/ContainmentPanel';
import { UncontainedBadge } from '../src/features/workspaces/ContainmentBadge';
import { MachineContainmentSwitch } from '../src/features/settings/ContainmentSettings';
import { CONTAINMENT_UNAVAILABLE_MESSAGE, normalizeError } from '../src/lib/errors';
import type { ContainmentStatus, Project } from '../src/types/domain';
import { demoProject } from '../src/app/demo';

const noop = () => undefined;

const blockedProfiles: ContainmentStatus['profiles'] = [
  { kind: 'inspect', enforcement: { state: 'blocked', reason: 'containment_unavailable' } },
  { kind: 'edit', enforcement: { state: 'blocked', reason: 'containment_unavailable' } },
  { kind: 'edit_with_network', enforcement: { state: 'blocked', reason: 'containment_unavailable' } },
  { kind: 'reduced_safety', enforcement: { state: 'blocked', reason: 'containment_unavailable' } },
];

const unsupported = ['workspace.read', 'workspace.write', 'temp.write', 'git.metadata', 'process.spawn', 'network'];

const blocked: ContainmentStatus = {
  projectId: demoProject.id,
  platform: 'windows',
  projectOptedIn: false,
  machineAllowed: true,
  profiles: blockedProfiles,
};

const reduced: ContainmentStatus = {
  projectId: demoProject.id,
  platform: 'windows',
  projectOptedIn: true,
  machineAllowed: true,
  profiles: [
    { kind: 'inspect', enforcement: { state: 'reduced_safety', unsupported: ['workspace.read', 'git.metadata', 'process.spawn'] } },
    { kind: 'edit', enforcement: { state: 'reduced_safety', unsupported: ['workspace.read', 'workspace.write', 'temp.write', 'git.metadata', 'process.spawn'] } },
    { kind: 'edit_with_network', enforcement: { state: 'reduced_safety', unsupported } },
    { kind: 'reduced_safety', enforcement: { state: 'reduced_safety', unsupported: [] } },
  ],
};

const machineDisabled: ContainmentStatus = {
  ...reduced,
  machineAllowed: false,
  profiles: blockedProfiles,
};

function panel(status: ContainmentStatus) {
  return renderToStaticMarkup(<ContainmentPanelView status={status} onEnable={noop} onDisable={noop} />);
}

function button(html: string, label: string) {
  return (html.match(/<button[\s\S]*?<\/button>/g) ?? []).find(part => part.includes(`>${label}</button>`)) ?? '';
}

function dialog(phrase: string) {
  return renderToStaticMarkup(<ReducedSafetyDialogView projectName="Northstar" phrase={phrase} busy={false} onPhrase={noop} onClose={noop} onConfirm={noop} />);
}

const uncontainedProject: Project = { ...demoProject, reducedSafetyOptIn: true };

describe('project containment panel', () => {
  it('blocks the project until reduced safety is turned on', () => {
    const html = panel(blocked);
    expect(html).toContain('Containment');
    expect(html).toContain('Agent commands');
    expect(html).toContain('Git and search');
    expect(html).toContain('New tasks');
    expect(html).toContain('This project is blocked until reduced safety is turned on for it.');
    expect(html).toContain('Unavailable');
    expect(button(html, 'Turn on reduced safety')).not.toContain('disabled');
  });

  it('reports process limits only and lists every unenforced capability', () => {
    const html = panel(reduced);
    expect(html).toContain('Reduced safety is on. Process limits only.');
    expect(html).toContain('Not enforced');
    for (const capability of unsupported) expect(html).toContain(capability);
    expect(html).not.toContain('>Enforced<');
    expect(button(html, 'Turn off reduced safety')).toContain('Turn off reduced safety');
  });

  it('reports that reduced safety is disabled on the machine', () => {
    const html = panel(machineDisabled);
    expect(html).toContain('Reduced safety is disabled on this machine.');
    expect(html).toContain('Settings → Permissions');
    expect(button(html, 'Turn on reduced safety')).toContain('disabled');
  });
});

describe('reduced safety confirmation dialog', () => {
  it('lists what reduced safety does not restrict', () => {
    const html = dialog('');
    expect(html).toContain('Writes outside the workspace');
    expect(html).toContain('Reads of credentials and other folders');
    expect(html).toContain('Network access');
    expect(html).toContain('Type “reduced safety” to confirm');
  });

  it('enables the confirm button only for the exact phrase', () => {
    expect(button(dialog(''), 'Turn on reduced safety')).toContain('disabled');
    expect(button(dialog('reduced'), 'Turn on reduced safety')).toContain('disabled');
    expect(button(dialog('Reduced safety'), 'Turn on reduced safety')).toContain('disabled');
    expect(button(dialog('reduced safety '), 'Turn on reduced safety')).toContain('disabled');
    expect(button(dialog(' reduced safety'), 'Turn on reduced safety')).toContain('disabled');
    expect(button(dialog('reduced safety'), 'Turn on reduced safety')).not.toContain('disabled');
  });
});

describe('reduced safety machine switch', () => {
  it('shows the machine-level setting and its checked state', () => {
    const on = renderToStaticMarkup(<MachineContainmentSwitch allowed busy={false} disabled={false} onChange={noop} />);
    expect(on).toContain('Allow reduced safety on this machine');
    expect(on).toContain('role="switch"');
    expect(on).toMatch(/checked=""/);

    const off = renderToStaticMarkup(<MachineContainmentSwitch allowed={false} busy={false} disabled={false} onChange={noop} />);
    expect(off).not.toMatch(/checked=""/);
  });
});

describe('uncontained badge', () => {
  it('marks tasks whose project opted into reduced safety', () => {
    expect(renderToStaticMarkup(<UncontainedBadge project={uncontainedProject} />)).toContain('Uncontained');
    expect(renderToStaticMarkup(<UncontainedBadge project={demoProject} />)).toBe('');
  });
});

describe('blocked command wording', () => {
  it('uses one exact message for containment_unavailable', () => {
    expect(CONTAINMENT_UNAVAILABLE_MESSAGE).toBe('Command execution is unavailable: Octran Code cannot enforce this task’s containment profile on this system. File edits still work.');
    expect(normalizeError({ code: 'containment_unavailable', message: 'backend detail (containment_unavailable)' }).message).toBe(CONTAINMENT_UNAVAILABLE_MESSAGE);
  });
});
