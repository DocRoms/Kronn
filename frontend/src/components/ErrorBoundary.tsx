import { Component } from 'react';
import type { ReactNode, ErrorInfo } from 'react';

interface Props {
  children: ReactNode;
  mode?: 'fullscreen' | 'zone';
  label?: string;
}

interface State {
  error: Error | null;
}

const ERROR_MESSAGES: Record<string, string> = {
  fr: 'Une erreur est survenue.',
  en: 'Something went wrong.',
  es: 'Algo salió mal.',
};

function getErrorMessage(): string {
  try {
    const stored = localStorage.getItem('kronn:ui-locale');
    if (stored && stored in ERROR_MESSAGES) return ERROR_MESSAGES[stored];
  } catch { /* ignore */ }
  return ERROR_MESSAGES.fr;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('Uncaught error:', error, info);
  }

  render() {
    if (!this.state.error) {
      return this.props.children;
    }

    const mode = this.props.mode ?? 'fullscreen';

    if (mode === 'zone') {
      return (
        <div style={{
          display: 'flex', alignItems: 'center', justifyContent: 'center', flexDirection: 'column', gap: 12,
          padding: '40px 20px', textAlign: 'center',
          background: 'rgba(var(--kr-error-rgb), 0.05)', border: '1px solid rgba(var(--kr-error-rgb), 0.15)', borderRadius: 12,
          margin: 16,
        }}>
          <span style={{ color: 'var(--kr-error)', fontSize: 14, fontFamily: 'JetBrains Mono, monospace', fontWeight: 600 }}>
            {this.props.label ? `${this.props.label} — ` : ''}{getErrorMessage()}
          </span>
          <pre style={{ color: 'var(--kr-text-faint)', fontSize: 11, maxWidth: '60vw', overflow: 'auto', margin: 0 }}>
            {this.state.error.message}
          </pre>
          <button
            onClick={() => this.setState({ error: null })}
            style={{
              padding: '6px 14px', cursor: 'pointer', background: 'var(--kr-accent-bg-hover)',
              color: 'var(--kr-accent-ink)', border: '1px solid var(--kr-accent-border)', borderRadius: 4,
              fontFamily: 'JetBrains Mono, monospace', fontSize: 12,
            }}
          >
            Retry
          </button>
        </div>
      );
    }

    return (
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100vh', flexDirection: 'column', gap: 16 }}>
        <span style={{ color: 'var(--kr-error)', fontSize: 15, fontFamily: 'JetBrains Mono, monospace' }}>
          {getErrorMessage()}
        </span>
        <pre style={{ color: 'var(--kr-text-tertiary)', fontSize: 12, maxWidth: '80vw', overflow: 'auto' }}>
          {this.state.error.message}
        </pre>
        <button onClick={() => window.location.reload()} style={{ padding: '8px 16px', cursor: 'pointer', background: 'var(--kr-accent)', color: 'var(--kr-text-on-accent)', border: 'none', borderRadius: 4, fontFamily: 'JetBrains Mono, monospace' }}>
          Reload
        </button>
      </div>
    );
  }
}
