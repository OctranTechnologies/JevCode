import { useEffect, useRef } from 'react';
import { ArrowRight, FileText, Folder, GitBranch, ShieldCheck, Terminal, UserRound } from 'lucide-react';
import { Brand } from '../../components/Brand';
import type { AgentSession } from '../../types/domain';

const suggestions = [
  { icon: Folder, label: 'Explore this project', prompt: 'Explore this project and explain its top-level structure.' },
  { icon: FileText, label: 'Explain the architecture', prompt: 'Read the README and explain the architecture.' },
  { icon: GitBranch, label: 'Review Git status', prompt: 'Review Git status for this project.' },
];
export function Conversation({ session, onSuggestion, onPermission, permissionBusy }: { session?: AgentSession; onSuggestion: (prompt: string) => void; onPermission: (approved: boolean) => void; permissionBusy: boolean }) {
  const end = useRef<HTMLDivElement>(null);
  const messages = session?.messages.filter(message => message.role !== 'system') ?? [];
  useEffect(() => { end.current?.scrollIntoView({ behavior: 'instant', block: 'end' }); }, [session?.updatedAt]);
  if (!messages.length) return <div className="welcome"><Brand large /><h1>What are we building?</h1><p>Explore a project, understand the code,<br className="wide-break" /> and plan your next change.</p><div className="suggestions">{suggestions.map(({ icon: Icon, label, prompt }) => <button key={label} onClick={() => onSuggestion(prompt)}><Icon size={19} /><span>{label}</span><ArrowRight size={17} /></button>)}</div></div>;
  return <div className="conversation" aria-label="Agent conversation" aria-live="polite" aria-relevant="additions text">
    {messages.map(message => <article className={`message message-${message.role}`} key={message.id}>
      <div className="message-avatar">{message.role === 'user' ? <UserRound size={16} /> : message.role === 'tool' ? <Terminal size={16} /> : <Brand />}</div>
      <div className="message-body"><div className="message-heading">{message.role === 'user' ? 'You' : message.role === 'tool' ? message.toolResult?.name : 'JevCode'}<time dateTime={message.createdAt}>{new Date(message.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time></div>
        {message.role === 'tool' ? <details className={message.toolResult?.isError ? 'tool-output tool-error' : 'tool-output'}><summary>{message.toolResult?.isError ? 'Tool could not complete' : 'Tool completed'}<span>{message.toolResult?.durationMs} ms</span></summary><pre>{message.content}</pre></details> : <div className="message-content">{message.content}</div>}
        {message.toolCalls.map(call => <div className="tool-call" key={call.id}><Terminal size={14} /><span>{call.name}</span><code>{JSON.stringify(call.arguments)}</code></div>)}
      </div>
    </article>)}
    {session?.pendingToolCall && <div className="permission-request"><ShieldCheck size={21} /><div><h2>Allow this tool call?</h2><p><strong>{session.pendingToolCall.name}</strong> will run in this project.</p><pre>{JSON.stringify(session.pendingToolCall.arguments, null, 2)}</pre><div className="permission-actions"><button className="secondary-button" disabled={permissionBusy} onClick={() => onPermission(false)}>Deny</button><button className="primary-button" disabled={permissionBusy} onClick={() => onPermission(true)}>Allow once</button></div></div></div>}
    {session?.status === 'running' && <div className="run-indicator" role="status"><span className="status-dot pulse" />JevCode is working…</div>}
    {session?.error && <p className="inline-error" role="alert">{session.error}</p>}
    <div ref={end} />
  </div>;
}
