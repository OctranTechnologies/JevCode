import { useEffect, useMemo, useRef } from 'react';
import {
  AlertCircle, ArrowUpRight, Check, ChevronDown, CircleDot, CircleHelp, Clock3, FileCode2, GitBranch,
  LoaderCircle, Shield, Sparkles, Terminal, UserRound,
} from 'lucide-react';
import { Brand } from '../../components/Brand';
import type { AgentSession, AgentMessage, PermissionResolution } from '../../types/domain';

const suggestions = [
  { icon: FileCode2, label: 'Explain this project', prompt: 'Explore this project and explain its main architecture.' },
  { icon: GitBranch, label: 'Review recent changes', prompt: 'Review the current Git status and summarize the changes.' },
  { icon: Terminal, label: 'Find a bug', prompt: 'Trace the main request flow and look for a likely source of bugs.' },
];

export function Conversation({
  session, projectName, demo = false, streamingText = '', toolOutput, onSuggestion, onPermission, onRetry, permissionBusy,
}: {
  session?: AgentSession;
  projectName?: string;
  demo?: boolean;
  streamingText?: string;
  toolOutput?: Record<string, { stdout: string; stderr: string }>;
  onSuggestion: (prompt: string) => void;
  onPermission: (resolution: PermissionResolution) => void;
  onRetry: () => void;
  permissionBusy: boolean;
}) {
  const end = useRef<HTMLDivElement>(null);
  const messages = useMemo(() => session?.messages.filter(message => message.role !== 'system') ?? [], [session?.messages]);
  const completedCalls = useMemo(() => new Set(messages.flatMap(message => message.toolResult ? [message.toolResult.toolCallId] : [])), [messages]);
  useEffect(() => { end.current?.scrollIntoView({ behavior: 'smooth', block: 'end' }); }, [session?.updatedAt]);

  return <div className="conversation-scroll" aria-label="Task conversation">
    {demo && <div className="sample-notice"><Sparkles size={13} /><span><strong>Sample task</strong> · Activity, terminal output, and diff are illustrative. No files were changed.</span></div>}
    {messages.length === 0 ? <div className="task-empty-state">
      <div className="empty-state-mark"><Brand large /></div>
      <h1>{projectName ? `What should JevCode do in ${projectName}?` : 'Start with a project'}</h1>
      <p>{projectName ? 'Describe the outcome you want. JevCode will inspect the project and keep you in control.' : 'Open a local folder to give your agent a place to work.'}</p>
      {projectName ? <div className="starter-prompts" aria-label="Suggested tasks">{suggestions.map(({ icon: Icon, label, prompt }) => <button key={label} onClick={() => onSuggestion(prompt)}><Icon size={15} /><span>{label}</span><ArrowUpRight size={13} className="starter-prompt-arrow" /></button>)}</div> : <div className="empty-state-note"><Shield size={15} /><span>Project files stay on this device. You approve actions that need it.</span></div>}
    </div> : <div className="task-timeline" aria-live="polite" aria-relevant="additions text">
      {messages.map(message => <TimelineMessage key={message.id} message={message} completedCalls={completedCalls} toolOutput={toolOutput} />)}
      {session?.pendingToolCall && <div className="approval-card" role="group" aria-labelledby="approval-heading">
        <div className="approval-icon"><Shield size={17} /></div>
        <div className="approval-content"><div className="approval-title-row"><h2 id="approval-heading">Permission needed</h2><span className="approval-required">Review before continuing</span></div>
          <p><strong>{session.pendingPermission?.categories.map(category => category.replaceAll('_', ' ')).join(' · ') ?? session.pendingToolCall.name}</strong> in <code>{projectName ?? 'this project'}</code></p>
          <pre>{session.pendingPermission?.summary ?? session.pendingToolCall.name}</pre>
          <p className="approval-reason">{session.pendingPermission?.reason ?? 'This action needs your approval.'}</p>
          <div className="approval-actions">
            <button className="button-quiet" disabled={permissionBusy} onClick={() => onPermission('deny')}>Deny</button>
            {session.pendingPermission?.canAlwaysAllow && <>
              <button className="button-quiet" disabled={permissionBusy} onClick={() => onPermission('allow_session')}>Allow for this session</button>
              <button className="button-quiet" disabled={permissionBusy} onClick={() => onPermission('always_allow_for_project')}>Always allow for this project</button>
            </>}
            <button className="button-primary" disabled={permissionBusy} onClick={() => onPermission('allow_once')}>{permissionBusy ? 'Applying…' : 'Allow once'}</button>
          </div>
          {!session.pendingPermission?.canAlwaysAllow && <small className="approval-scrutiny-note">This action will be reviewed again if requested later.</small>}
        </div>
      </div>}
      {session?.pendingUserInput && <div className="agent-question-card" role="status"><CircleHelp size={15} /><div><strong>JevCode needs an answer</strong><p>{String(session.pendingUserInput.arguments.question ?? 'Please provide the requested information.')}</p><small>Reply in the composer to continue this task.</small></div></div>}
      {session?.status && ['queued', 'planning', 'working'].includes(session.status) && <div className="agent-working" role="status"><LoaderCircle size={15} className="spin" /><span>{session.status === 'planning' ? 'Collecting project context' : 'JevCode is working through this task'}</span><span className="working-dots"><i /><i /><i /></span></div>}
      {session?.activityEvents.filter(event => ['plan', 'progress', 'file_inspected', 'search_performed', 'command_executed', 'file_edited', 'test_run'].includes(event.kind)).map(event => <div className="agent-activity-summary" key={event.id}><CircleDot size={12} /><span>{event.summary}</span><time dateTime={event.createdAt}>{formatTime(event.createdAt)}</time></div>)}
      {streamingText && <article className="timeline-agent is-streaming"><div className="timeline-agent-mark"><Brand /></div><div className="timeline-agent-content"><div className="timeline-meta"><strong>JevCode</strong><span className="streaming-label"><i />Streaming</span></div><div className="agent-prose">{streamingText}</div></div></article>}
      {session?.status === 'failed' && session.error && <div className="task-error-state" role="alert"><AlertCircle size={16} /><div><strong>The task stopped</strong><p>{session.error}</p><button onClick={onRetry}>Retry this request</button></div></div>}
      <div ref={end} />
    </div>}
  </div>;
}

function TimelineMessage({ message, completedCalls, toolOutput }: { message: AgentMessage; completedCalls: Set<string>; toolOutput?: Record<string, { stdout: string; stderr: string }> }) {
  if (message.role === 'tool') return <details className={`activity-result${message.toolResult?.isError ? ' is-error' : ''}`}>
    <summary><span className="activity-result-icon">{message.toolResult?.isError ? <AlertCircle size={14} /> : <Check size={14} />}</span><span className="activity-result-name">{message.toolResult?.name ?? 'Tool activity'}</span><span className="activity-result-state">{message.toolResult?.isError ? 'Needs attention' : 'Completed'}</span><span className="activity-result-duration">{formatDuration(message.toolResult?.durationMs ?? 0)}</span><ChevronDown size={13} className="activity-chevron" /></summary>
    <pre>{message.content}</pre>
  </details>;

  if (message.role === 'user') return <article className="timeline-user">
    <div className="timeline-user-mark"><UserRound size={14} /></div><div className="timeline-user-content"><div className="timeline-meta"><strong>You</strong><time dateTime={message.createdAt}>{formatTime(message.createdAt)}</time></div><p>{message.content}</p></div>
  </article>;

  const hasCalls = message.toolCalls.length > 0;
  return <article className="timeline-agent">
    <div className="timeline-agent-mark"><Brand /></div><div className="timeline-agent-content">
      <div className="timeline-meta"><strong>JevCode</strong><time dateTime={message.createdAt}>{formatTime(message.createdAt)}</time>{hasCalls && <span className="activity-label"><CircleDot size={11} />Activity</span>}</div>
      {message.content && <div className="agent-prose">{message.content}</div>}
      {hasCalls && <div className="activity-list">{message.toolCalls.map(call => {
        const done = completedCalls.has(call.id);
        const output = call.name === 'run_command' ? toolOutput?.[call.id] : undefined;
        return <div className="activity-call-wrap" key={call.id}><div className="activity-call"><span className={`activity-call-icon${done ? ' is-done' : ''}`}>{done ? <Check size={12} /> : <Clock3 size={12} />}</span><code>{call.name}</code><span className="activity-call-summary">{formatArguments(call.arguments)}</span><span className="activity-call-state">{done ? 'Done' : output ? 'Running' : 'Queued'}</span></div>
          {!done && output && <div className="terminal-stream" aria-label="Streaming command output">{output.stdout && <pre><small>stdout</small>{output.stdout}</pre>}{output.stderr && <pre className="is-stderr"><small>stderr</small>{output.stderr}</pre>}</div>}
        </div>;
      })}</div>}
    </div>
  </article>;
}

function formatTime(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

function formatDuration(duration: number) {
  return duration < 1000 ? `${duration} ms` : `${(duration / 1000).toFixed(1)} s`;
}

function formatArguments(args: Record<string, unknown>) {
  const path = typeof args.path === 'string' ? args.path : null;
  return path ?? Object.keys(args).join(', ');
}
