import type { WorkflowTrigger, WatchTrigger } from '../types/generated';

export type WatchDetectionMode = 'Validators' | 'Body' | 'JsonPath';

/** Editable form of a `Watch` trigger. */
export interface WatchDraft {
  source: 'quick_api' | 'api';
  quickApiId: string;
  pluginSlug: string;
  configId: string;
  endpointPath: string;
  /** Not editable in the wizard; kept so an edit does not drop it. */
  apiQuery: Record<string, string> | null;
  interval: string;
  timezone: string;
  detection: WatchDetectionMode;
  jsonPath: string;
}

export const DEFAULT_WATCH_INTERVAL = '*/5 * * * *';

/** IANA zones offered first; any other IANA name can be typed. */
export const COMMON_TIMEZONES = [
  'UTC', 'Europe/Paris', 'Europe/London', 'Europe/Berlin', 'America/New_York',
  'America/Chicago', 'America/Los_Angeles', 'Asia/Tokyo', 'Asia/Shanghai', 'Australia/Sydney',
];

export function watchDraftFrom(trigger: WorkflowTrigger | null | undefined): WatchDraft {
  const w: Partial<WatchTrigger> = trigger?.type === 'Watch' ? trigger : {};
  const detection = w.detection;
  const query = w.api_query
    ? Object.fromEntries(Object.entries(w.api_query).filter((e): e is [string, string] => typeof e[1] === 'string'))
    : null;
  return {
    source: w.quick_api_id ? 'quick_api' : 'api',
    quickApiId: w.quick_api_id ?? '',
    pluginSlug: w.api_plugin_slug ?? '',
    configId: w.api_config_id ?? '',
    endpointPath: w.api_endpoint_path ?? '',
    apiQuery: query,
    interval: w.interval ?? DEFAULT_WATCH_INTERVAL,
    timezone: w.timezone ?? '',
    detection: detection?.type ?? 'Validators',
    jsonPath: detection?.type === 'JsonPath' ? detection.path : '',
  };
}

export function buildWatchTrigger(d: WatchDraft): WorkflowTrigger {
  const trigger: { type: 'Watch' } & WatchTrigger = {
    type: 'Watch',
    interval: d.interval.trim(),
    detection: d.detection === 'JsonPath'
      ? { type: 'JsonPath', path: d.jsonPath.trim() }
      : { type: d.detection },
  };
  if (d.source === 'quick_api') {
    if (d.quickApiId) trigger.quick_api_id = d.quickApiId;
  } else {
    if (d.pluginSlug) trigger.api_plugin_slug = d.pluginSlug;
    if (d.configId) trigger.api_config_id = d.configId;
    if (d.endpointPath.trim()) trigger.api_endpoint_path = d.endpointPath.trim();
  }
  // Not editable here: a saved query override survives any other edit.
  if (d.apiQuery && Object.keys(d.apiQuery).length > 0) trigger.api_query = d.apiQuery;
  if (d.timezone.trim()) trigger.timezone = d.timezone.trim();
  return trigger;
}

/** A Cron trigger; an empty timezone is omitted so the schedule stays in UTC. */
export function buildCronTrigger(schedule: string, timezone: string): WorkflowTrigger {
  const tz = timezone.trim();
  return tz ? { type: 'Cron', schedule, timezone: tz } : { type: 'Cron', schedule };
}
