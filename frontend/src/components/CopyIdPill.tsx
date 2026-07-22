import { useEffect, useRef, useState } from 'react';
import { Check } from 'lucide-react';
import './CopyIdPill.css';

interface CopyIdPillProps {
  id: string;
  label?: string;
  title?: string;
  className?: string;
}

export function CopyIdPill({ id, label, title, className = '' }: CopyIdPillProps) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => {
    if (resetTimer.current) clearTimeout(resetTimer.current);
  }, []);

  const copyId = async () => {
    try {
      await navigator.clipboard.writeText(id);
    } catch {
      const textarea = document.createElement('textarea');
      textarea.value = id;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      textarea.remove();
    }

    setCopied(true);
    if (resetTimer.current) clearTimeout(resetTimer.current);
    resetTimer.current = setTimeout(() => setCopied(false), 1500);
  };

  return (
    <button
      type="button"
      className={`copy-id-pill ${className}`.trim()}
      data-copied={copied}
      onClick={(event) => {
        event.stopPropagation();
        void copyId();
      }}
      title={title ?? id}
      aria-label={title ?? id}
    >
      {copied && <Check size={8} aria-hidden="true" />}
      <span>{label ?? `#${id.slice(0, 8)}`}</span>
    </button>
  );
}
