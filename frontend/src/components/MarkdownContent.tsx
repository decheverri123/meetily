import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

/**
 * Renders markdown (GFM) with the standard compact `prose` styling shared by
 * the live-insights panel and action chip popovers.
 */
export function MarkdownContent({ children }: { children: string }) {
  return (
    <div className="prose prose-sm max-w-none prose-p:my-2 prose-ul:my-2 prose-headings:my-2">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}
