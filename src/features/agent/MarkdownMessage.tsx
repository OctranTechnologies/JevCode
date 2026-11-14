import { isValidElement, useState } from 'react';
import type { ReactNode } from 'react';
import { Check, Copy } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

/** Shared safe Markdown rendering for both sides of the conversation. */
export function MarkdownMessage({ content, className = '' }: { content: string; className?: string }) {
  return <div className={`message-markdown ${className}`.trim()}>
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ pre: CodeBlock }}>{content}</ReactMarkdown>
  </div>;
}

function CodeBlock({ children }: { children?: ReactNode }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(textOf(children));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch { /* The clipboard can be unavailable in some webviews; the code stays selectable. */ }
  }
  return <div className="code-block">
    <button type="button" className="code-copy-button" onClick={copy}>{copied ? <Check size={12} /> : <Copy size={12} />}<span>{copied ? 'Copied' : 'Copy'}</span></button>
    <pre>{children}</pre>
  </div>;
}

function textOf(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return '';
}
