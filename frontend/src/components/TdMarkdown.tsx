import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';

const REMARK_PLUGINS = [remarkGfm];

/** Kept outside the component so a re-render does not remount the Markdown.
 *  Same safety as the repository resources' viewer: no raw HTML, only a web
 *  link is followed and an image is reduced to its caption. */
const MARKDOWN_COMPONENTS: Components = {
  a: ({ href, children }) => (
    href && /^https?:\/\//i.test(href)
      ? <a href={href} target="_blank" rel="noopener noreferrer">{children}</a>
      : <span>{children}</span>
  ),
  img: ({ alt }) => <span>{alt}</span>,
};

/** A sheet's free text (`**Critical**`, `- ` bullets) rendered as Markdown. */
export function TdMarkdown({ text }: { text: string }) {
  return (
    <div className="td-markdown" data-testid="td-markdown">
      <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={MARKDOWN_COMPONENTS}>{text}</ReactMarkdown>
    </div>
  );
}
