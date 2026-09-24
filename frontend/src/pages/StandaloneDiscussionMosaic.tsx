import { memo, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { ArrowUpRight, ChevronsDown, MessageSquareReply, RefreshCw } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useT } from '../lib/I18nContext';
import { AGENT_LABELS } from '../lib/constants';
import { useDiscussionMonitor } from '../hooks/useDiscussionMonitor';
import { useToast } from '../hooks/useToast';
import { DiscussionMosaicComposer } from '../components/DiscussionMosaicComposer';
import { discussionMosaicUrl, type DiscussionMosaicLayout } from '../lib/discussion-mosaic-navigation';
import { livePageMosaicLayouts, standaloneDiscussionUrl } from '../lib/live-page-navigation';
import type { DiscussionMonitorItem, DiscussionMonitorMessage } from '../types/generated';
import './StandaloneLivePageMosaic.css';
import './StandaloneDiscussionMosaic.css';

function timeLabel(value: string | number): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

const MessagePreview = memo(function MessagePreview(message: DiscussionMonitorMessage & { partial?: boolean }) {
  const { t } = useT();
  const author = message.author_pseudo || (message.agent_type ? AGENT_LABELS[message.agent_type] : '') || message.role;
  return <section className="discussion-mosaic-message" data-role={message.role} data-partial={message.partial || undefined}>
    <div className="discussion-mosaic-message-meta">
      <strong>{author}{message.author_cli_ordinal != null && ` · CLI ${message.author_cli_ordinal}`}</strong>
      {message.model && <span>{message.model}</span>}
      {message.channel === 'note' && <span>{t('disc.mosaic.note')}</span>}
      <time dateTime={message.timestamp}>{timeLabel(message.timestamp)}</time>
    </div>
    {message.partial && <small>{t('disc.mosaic.checkpoint')}</small>}
    {message.partial && message.truncated && <small>{t('disc.mosaic.tail')}</small>}
    <div className="discussion-mosaic-message-text">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: props => <a {...props} target="_blank" rel="noopener noreferrer" />, img: props => <span>{props.alt}</span> }}>{message.content}</ReactMarkdown>
    </div>
    {!message.partial && message.truncated && <small>{t('disc.mosaic.truncated')}</small>}
  </section>;
});

const DiscussionTile = memo(function DiscussionTile({ id, item, error, selected, onSelect }: {
  id: string; item?: DiscussionMonitorItem; error: boolean; selected: boolean; onSelect: (id: string) => void;
}) {
  const { t } = useT();
  const scroll = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);
  const preview = item?.preview;
  useLayoutEffect(() => {
    if (follow && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight;
  }, [preview, follow]);
  const plan = preview?.plan;
  const total = plan ? plan.done + plan.ready + plan.blocked + plan.in_progress + plan.ideas : 0;
  const status = preview?.pending_question_count ? t('disc.mosaic.question')
    : preview?.progress_phase === 'upstream_wait' ? t('disc.mosaic.upstream')
      : preview?.agent_running ? t('disc.mosaic.running')
        : preview?.awaiting_agent ? t('disc.mosaic.queued') : t('disc.mosaic.ready');
  return <article className="standalone-live-page-mosaic-tile discussion-mosaic-tile" aria-label={preview?.title || id}
    data-selected={selected || undefined} onClick={() => onSelect(id)}>
    <header className="discussion-mosaic-tile-header">
      <div>
        <h2><a href={standaloneDiscussionUrl(id)} target="_blank" rel="noopener noreferrer">{preview?.title || id}<ArrowUpRight size={15} /></a></h2>
        <div className="discussion-mosaic-identity"><code title={id}>#{id.slice(0, 8)}</code>
          {preview?.agent && <span>{preview.connection_name || AGENT_LABELS[preview.agent]}</span>}
        </div>
      </div>
      <div className="discussion-mosaic-tile-actions">
        {preview && <span className="discussion-mosaic-status" data-active={preview.awaiting_agent || preview.agent_running}>{status}</span>}
        <button type="button" className="discussion-mosaic-select" aria-pressed={selected}
          aria-label={t('disc.mosaic.replyIn', preview?.title || id)} title={t('disc.mosaic.replyIn', preview?.title || id)}
          onClick={event => { event.stopPropagation(); onSelect(id); }}><MessageSquareReply size={14} /></button>
      </div>
    </header>
    {plan && <div className="discussion-mosaic-plan">
      {total > 0 ? <>
        <span>{t('disc.mosaic.plan', plan.done, total)}</span>
        <progress value={plan.done} max={total} aria-label={t('disc.mosaic.plan', plan.done, total)} />
        <span>{t('disc.mosaic.planDetails', plan.in_progress, plan.ready, plan.blocked, plan.ideas)}</span>
      </> : <span>{t('disc.mosaic.noPlan')}</span>}
      {plan.later > 0 && <span>{t('disc.mosaic.later', plan.later)}</span>}
    </div>}
    <div ref={scroll} className="discussion-mosaic-scroll" tabIndex={0} aria-label={t('disc.mosaic.messages', preview?.title || id)}
      onScroll={event => {
        const element = event.currentTarget;
        setFollow(element.scrollHeight - element.scrollTop - element.clientHeight < 48);
      }}>
      {item?.error ? <p role="alert">{t(item.error === 'not_found' ? 'disc.mosaic.notFound' : 'disc.mosaic.unavailable')}</p>
        : !preview ? <p role={error ? 'alert' : 'status'}>{t(error ? 'disc.mosaic.unavailable' : 'disc.mosaic.loading')}</p>
          : <>
            <p className="discussion-mosaic-hint">{t('disc.mosaic.previewHint')}</p>
            {!preview.messages.length && !preview.partial_response && <p>{t('disc.mosaic.empty')}</p>}
            {preview.messages.map(message => <MessagePreview key={message.id} {...message} />)}
            {preview.partial_response && <MessagePreview {...preview.partial_response} partial />}
          </>}
    </div>
    {!follow && <button type="button" className="discussion-mosaic-follow" onClick={() => setFollow(true)}><ChevronsDown size={14} />{t('disc.mosaic.follow')}</button>}
  </article>;
});

const layoutKeys: Record<DiscussionMosaicLayout, string> = {
  auto: 'pages.mosaic.layout.auto', 'two-columns': 'pages.mosaic.layout.twoColumns', 'two-rows': 'pages.mosaic.layout.twoRows',
  'three-top': 'pages.mosaic.layout.threeTop', 'three-bottom': 'pages.mosaic.layout.threeBottom',
  'three-left': 'pages.mosaic.layout.threeLeft', 'three-right': 'pages.mosaic.layout.threeRight',
};

export function StandaloneDiscussionMosaic({ discussionIds, layout: initialLayout }: { discussionIds: string[]; layout: DiscussionMosaicLayout }) {
  const { t } = useT();
  const layoutId = useId();
  const [layout, setLayout] = useState(initialLayout);
  const { items, error, updatedAt, connectionState, refresh } = useDiscussionMonitor(discussionIds);
  const { toast, ToastContainer } = useToast();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const selectedTitle = selectedId ? items.find(item => item.id === selectedId)?.preview?.title || selectedId : '';
  useEffect(() => {
    const previous = document.title;
    document.title = `${t('disc.mosaic.title')} · Kronn`;
    return () => { document.title = previous; };
  }, [t]);
  return <main className="discussion-mosaic">
    <header className="discussion-mosaic-toolbar">
      <h1>{t('disc.mosaic.title')} <span>{discussionIds.length}</span></h1>
      <label htmlFor={layoutId}>{t('pages.mosaic.chooseLayout')}</label>
      <select id={layoutId} value={layout} onChange={event => {
        const next = event.target.value as DiscussionMosaicLayout;
        setLayout(next);
        window.history.replaceState(window.history.state, '', discussionMosaicUrl(discussionIds, next));
      }}>{livePageMosaicLayouts(discussionIds.length).map(option => <option key={option} value={option}>{t(layoutKeys[option])}</option>)}</select>
      <span className="discussion-mosaic-sync" role={error ? 'alert' : 'status'}>
        {error ? t('disc.mosaic.refreshError') : t(connectionState === 'connected' ? 'disc.mosaic.live' : 'disc.mosaic.reconnecting')}
        {updatedAt && <time>{timeLabel(updatedAt)}</time>}
      </span>
      <button type="button" onClick={refresh} aria-label={t('disc.mosaic.refresh')} title={t('disc.mosaic.refresh')}><RefreshCw size={16} /></button>
    </header>
    <div className="standalone-live-page-mosaic discussion-mosaic-grid" data-layout={layout} data-count={discussionIds.length}>
      {discussionIds.map(id => <DiscussionTile key={id} id={id} item={items.find(item => item.id === id)} error={error}
        selected={id === selectedId} onSelect={setSelectedId} />)}
    </div>
    <DiscussionMosaicComposer discussionId={selectedId} title={selectedTitle} toast={toast} />
    <ToastContainer />
  </main>;
}
