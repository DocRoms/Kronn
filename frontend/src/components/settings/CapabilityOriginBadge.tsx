import { ExternalLink, Sparkles, UserRound } from 'lucide-react';

export type CapabilityOrigin = 'kronn' | 'personal' | 'external';

interface CapabilityOriginBadgeProps {
  origin: CapabilityOrigin;
  t: (key: string, ...args: (string | number)[]) => string;
}

const ORIGIN_META = {
  kronn: { label: 'config.originKronn', Icon: Sparkles },
  personal: { label: 'config.originPersonal', Icon: UserRound },
  external: { label: 'config.originExternal', Icon: ExternalLink },
} as const;

export function CapabilityOriginBadge({ origin, t }: CapabilityOriginBadgeProps) {
  const { label, Icon } = ORIGIN_META[origin];

  return (
    <span className="set-capability-origin-badge" data-origin={origin}>
      <Icon size={10} aria-hidden="true" />
      {t(label)}
    </span>
  );
}
