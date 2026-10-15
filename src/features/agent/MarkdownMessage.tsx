import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

/** Shared safe Markdown rendering for both sides of the conversation. */
export function MarkdownMessage({ content, className = '' }: { content: string; className?: string }) {
  return <div className={`message-markdown ${className}`.trim()}>
    <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
  </div>;
}
