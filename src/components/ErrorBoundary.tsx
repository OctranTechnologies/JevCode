import { Component, type ReactNode } from 'react';
import { AlertTriangle } from 'lucide-react';
import { logEvent } from '../lib/logger';

export class ErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch() { logEvent('react_boundary_failure', 'error'); }
  render() {
    if (this.state.failed) return <main className="fatal-state"><AlertTriangle size={32} /><h1>JevCode needs a fresh start</h1><p>The interface encountered an error. Your saved sessions are stored locally.</p><button className="primary-button" onClick={() => window.location.reload()}>Reload application</button></main>;
    return this.props.children;
  }
}
