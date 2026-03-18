import { Component } from 'react';
import type { ReactNode, ErrorInfo } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
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
    if (this.state.error) {
      return (
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', height: '100vh', flexDirection: 'column', gap: 16 }}>
          <span style={{ color: '#ff4444', fontSize: 15, fontFamily: 'JetBrains Mono, monospace' }}>
            Something went wrong.
          </span>
          <pre style={{ color: 'rgba(255,255,255,0.5)', fontSize: 12, maxWidth: '80vw', overflow: 'auto' }}>
            {this.state.error.message}
          </pre>
          <button onClick={() => window.location.reload()} style={{ padding: '8px 16px', cursor: 'pointer', background: '#c8ff00', color: '#000', border: 'none', borderRadius: 4, fontFamily: 'JetBrains Mono, monospace' }}>
            Reload
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
