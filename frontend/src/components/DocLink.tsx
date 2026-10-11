import { useContext, type MouseEvent, type ReactNode } from 'react';
import { DocLinkContext, resolveDocLink, slugifyHeading } from '../lib/docLinks';

/** Shared `a` renderer for project docs: no doc link ever reaches the browser as a navigation. */
export function DocLink({ href, children }: { href?: string; children?: ReactNode }) {
  const ctx = useContext(DocLinkContext);
  const target = ctx ? resolveDocLink(href, ctx.currentPath, ctx.index) : resolveDocLink(href, '', { files: new Set(), roots: [] });

  if (target.kind === 'external') {
    return <a href={target.href} target="_blank" rel="noopener noreferrer">{children}</a>;
  }
  if (target.kind === 'inert' || !ctx) return <span>{children}</span>;

  const follow = (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    ctx.onNavigate(target);
  };
  // Middle-click would open the app again in a new tab; swallow it instead.
  const swallow = (event: MouseEvent<HTMLAnchorElement>) => event.preventDefault();

  return (
    <a href={href} className="aidoc-internal-link" data-link-kind={target.kind} onClick={follow} onAuxClick={swallow}>
      {children}
    </a>
  );
}

function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  if (typeof node === 'object' && 'props' in node) {
    return textOf((node.props as { children?: ReactNode }).children);
  }
  return '';
}

/** Heading with the slug id that `#section` links scroll to. */
export function DocHeading({ level, children }: { level: 1 | 2 | 3 | 4 | 5 | 6; children?: ReactNode }) {
  const Tag = `h${level}` as const;
  const id = slugifyHeading(textOf(children)) || undefined;
  return <Tag id={id}>{children}</Tag>;
}
