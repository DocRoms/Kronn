// KT-1438 — Settings > Worktrees: what Kronn puts into every task worktree
// besides the tracked files. A fresh worktree lacks a project's generated,
// gitignored assets (a compiled `public/build`, a `public/images`); a project
// declares them as lane seeds and Kronn copies them from its main checkout.
// These three switches are the owner's global say on that: copying at all,
// adopting Kronn's own suggestions, and whether changing a project's seeds asks
// for the strong confirmation. Each change is a `POST /config/server`, which
// asks for the code itself once an authenticator is enrolled.
import { useEffect, useState } from 'react';
import { FolderInput } from 'lucide-react';
import { config as configApi } from '../../lib/api';
import { userError } from '../../lib/userError';
import type { ToastFn } from '../../hooks/useToast';
import '../../pages/SettingsPage.css';

interface WorktreesSectionProps {
  toast: ToastFn;
  t: (key: string, ...args: (string | number)[]) => string;
}

type SeedSwitch = 'lane_seeds_copy_enabled' | 'lane_seeds_adopt_suggested' | 'lane_seeds_strong_confirm';
type SeedSwitches = Record<SeedSwitch, boolean>;

const ROWS: { field: SeedSwitch; id: string; label: string; hint: string }[] = [
  { field: 'lane_seeds_copy_enabled', id: 'copy', label: 'settings.worktrees.copy', hint: 'settings.worktrees.copyHint' },
  { field: 'lane_seeds_adopt_suggested', id: 'adopt', label: 'settings.worktrees.adopt', hint: 'settings.worktrees.adoptHint' },
  { field: 'lane_seeds_strong_confirm', id: 'confirm', label: 'settings.worktrees.confirm', hint: 'settings.worktrees.confirmHint' },
];

export function WorktreesSection({ toast, t }: WorktreesSectionProps) {
  const [values, setValues] = useState<SeedSwitches | null>(null);
  const [saving, setSaving] = useState<SeedSwitch | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    configApi
      .getServerConfig()
      .then(cfg => {
        if (!live) return;
        setValues({
          lane_seeds_copy_enabled: cfg.lane_seeds_copy_enabled ?? true,
          lane_seeds_adopt_suggested: cfg.lane_seeds_adopt_suggested ?? false,
          lane_seeds_strong_confirm: cfg.lane_seeds_strong_confirm ?? true,
        });
      })
      .catch(cause => { if (live) setError(userError(cause)); });
    return () => { live = false; };
  }, []);

  const change = async (field: SeedSwitch) => {
    if (!values || saving) return;
    const next = !values[field];
    setSaving(field);
    setError(null);
    try {
      await configApi.setServerConfig({ [field]: next });
      setValues({ ...values, [field]: next });
      toast(t('settings.worktrees.saved'), 'success');
    } catch (cause) {
      // The switch stays where it was (a refused or cancelled confirmation too).
      setError(userError(cause));
    } finally {
      setSaving(null);
    }
  };

  return (
    <div id="settings-worktrees" className="set-card" data-testid="settings-worktrees">
      <div className="set-section">
        <div className="flex-row gap-4 set-section-header-lg">
          <FolderInput size={14} className="text-accent" />
          <span className="font-semibold text-lg">{t('settings.worktrees.title')}</span>
        </div>
        <p className="set-hint">{t('settings.worktrees.hint')}</p>
        {error && <p className="set-db-usage-error" role="alert" data-testid="worktrees-error">{error}</p>}
        {values && ROWS.map(row => {
          const on = values[row.field];
          return (
            <div key={row.field} className="flex-col gap-1" style={{ marginBottom: 12 }}>
              <div className="flex-row gap-4">
                <span className="label" style={{ marginBottom: 0 }}>{t(row.label)}</span>
                <button
                  type="button"
                  role="switch"
                  aria-label={t(row.label)}
                  aria-checked={on}
                  className="set-agent-access-switch"
                  style={{ marginLeft: 'auto' }}
                  disabled={saving !== null}
                  data-testid={`worktrees-${row.id}`}
                  onClick={() => void change(row.field)}
                >
                  <span className="set-toggle-track" data-on={on}>
                    <span className="set-toggle-thumb" data-on={on} style={{ left: on ? 16 : 1 }} />
                  </span>
                  <span className={on ? 'text-accent' : 'text-muted'}>
                    {on ? t('settings.worktrees.on') : t('settings.worktrees.off')}
                  </span>
                </button>
              </div>
              <p className="set-hint-xs">{t(row.hint)}</p>
            </div>
          );
        })}
      </div>
    </div>
  );
}
