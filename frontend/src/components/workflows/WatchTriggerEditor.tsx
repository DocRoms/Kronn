import { useT } from '../../lib/I18nContext';
import { COMMON_TIMEZONES, type WatchDraft, type WatchDetectionMode } from '../../lib/watchTrigger';
import type { QuickApi } from '../../types/generated';
import type { ApiPluginOption } from './ApiCallStepCard';

/** IANA timezone input; empty means UTC. Shared by the Cron and Watch triggers. */
export function TimezoneField({ id, value, onChange }: { id: string; value: string; onChange: (tz: string) => void }) {
  const { t } = useT();
  return (
    <div className="mb-4">
      <label className="wf-label" htmlFor={id}>{t('wiz.timezone')}</label>
      <input
        id={id}
        className="wf-input"
        list={`${id}-zones`}
        value={value}
        onChange={e => onChange(e.target.value)}
        placeholder="UTC"
        spellCheck={false}
      />
      <datalist id={`${id}-zones`}>
        {COMMON_TIMEZONES.map(zone => <option key={zone} value={zone} />)}
      </datalist>
      <p className="text-xs text-ghost mt-1">{t('wiz.timezoneUtcNote')}</p>
    </div>
  );
}

interface WatchTriggerEditorProps {
  value: WatchDraft;
  onChange: (next: WatchDraft) => void;
  availableQuickApis: QuickApi[];
  availableApiPlugins: ApiPluginOption[];
}

export function WatchTriggerEditor({ value, onChange, availableQuickApis, availableApiPlugins }: WatchTriggerEditorProps) {
  const { t } = useT();
  const set = (patch: Partial<WatchDraft>) => onChange({ ...value, ...patch });
  const selectedServer = availableApiPlugins.find(p => p.config.id === value.configId)?.server;
  const endpoints = selectedServer?.api_spec?.endpoints ?? [];
  const getQuickApis = availableQuickApis.filter(qa => (qa.api_method ?? 'GET').toUpperCase() === 'GET');

  return (
    <div data-testid="watch-trigger-editor">
      <p className="text-xs text-muted mb-4">{t('wiz.watchIntro')}</p>

      <label className="wf-label" htmlFor="wf-watch-source">{t('wiz.watchSource')}</label>
      <select
        id="wf-watch-source"
        className="wf-select mb-4"
        value={value.source}
        onChange={e => set({ source: e.target.value as WatchDraft['source'] })}
      >
        <option value="api">{t('wiz.watchSourceApi')}</option>
        <option value="quick_api">{t('wiz.watchSourceQuickApi')}</option>
      </select>

      {value.source === 'quick_api' ? (
        <>
          <label className="wf-label" htmlFor="wf-watch-qa">{t('wiz.watchQuickApi')}</label>
          <select
            id="wf-watch-qa"
            className="wf-select mb-4"
            value={value.quickApiId}
            onChange={e => set({ quickApiId: e.target.value })}
          >
            <option value="">—</option>
            {getQuickApis.map(qa => (
              <option key={qa.id} value={qa.id}>{qa.icon} {qa.name} — GET {qa.api_endpoint_path}</option>
            ))}
          </select>
        </>
      ) : (
        <div className="flex-row gap-4 mb-4">
          <div className="flex-1">
            <label className="wf-label" htmlFor="wf-watch-plugin">{t('wiz.watchApi')}</label>
            <select
              id="wf-watch-plugin"
              className="wf-select"
              value={value.configId}
              onChange={e => {
                const match = availableApiPlugins.find(p => p.config.id === e.target.value);
                set({ configId: e.target.value, pluginSlug: match?.server.id ?? '', endpointPath: '' });
              }}
            >
              <option value="">—</option>
              {availableApiPlugins.map(p => (
                <option key={p.config.id} value={p.config.id}>{p.server.name} — {p.config.label}</option>
              ))}
            </select>
          </div>
          <div className="flex-1">
            <label className="wf-label" htmlFor="wf-watch-endpoint">{t('wiz.watchEndpoint')}</label>
            <input
              id="wf-watch-endpoint"
              className="wf-input"
              list={selectedServer ? `wf-watch-endpoints-${selectedServer.id}` : undefined}
              value={value.endpointPath}
              onChange={e => set({ endpointPath: e.target.value })}
              placeholder="/repos/owner/repo/commits"
              spellCheck={false}
            />
            {selectedServer && (
              <datalist id={`wf-watch-endpoints-${selectedServer.id}`}>
                {endpoints.filter(ep => ep.method.toUpperCase() === 'GET').map(ep => (
                  <option key={ep.path} value={ep.path}>{ep.description.slice(0, 80)}</option>
                ))}
              </datalist>
            )}
          </div>
        </div>
      )}

      <label className="wf-label" htmlFor="wf-watch-interval">{t('wiz.watchInterval')}</label>
      <input
        id="wf-watch-interval"
        className="wf-input mb-1"
        style={{ fontFamily: 'var(--kr-font-mono)' }}
        value={value.interval}
        onChange={e => set({ interval: e.target.value })}
        placeholder="*/5 * * * *"
        spellCheck={false}
      />
      <p className="text-xs text-ghost mb-4">{t('wiz.watchIntervalHint')}</p>

      <TimezoneField id="wf-watch-timezone" value={value.timezone} onChange={timezone => set({ timezone })} />

      <label className="wf-label" htmlFor="wf-watch-detection">{t('wiz.watchDetection')}</label>
      <select
        id="wf-watch-detection"
        className="wf-select mb-1"
        value={value.detection}
        onChange={e => set({ detection: e.target.value as WatchDetectionMode })}
      >
        <option value="Validators">{t('wiz.watchDetectionValidators')}</option>
        <option value="Body">{t('wiz.watchDetectionBody')}</option>
        <option value="JsonPath">{t('wiz.watchDetectionJsonPath')}</option>
      </select>
      <p className="text-xs text-ghost mb-4">{t('wiz.watchDetectionHint')}</p>
      {value.detection === 'JsonPath' && (
        <>
          <label className="wf-label" htmlFor="wf-watch-jsonpath">{t('wiz.watchJsonPath')}</label>
          <input
            id="wf-watch-jsonpath"
            className="wf-input mb-4"
            style={{ fontFamily: 'var(--kr-font-mono)' }}
            value={value.jsonPath}
            onChange={e => set({ jsonPath: e.target.value })}
            placeholder="$.items[*].id"
            spellCheck={false}
          />
        </>
      )}
      <p className="text-xs text-ghost">{t('wiz.watchBaselineNote')}</p>
    </div>
  );
}
