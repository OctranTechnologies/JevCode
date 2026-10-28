import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { CircleAlert, CircleCheck, FlaskConical, KeyRound, LoaderCircle, RefreshCw, ShieldCheck, Trash2 } from 'lucide-react';
import { command, desktopAvailable } from '../../lib/ipc';
import type { DecisionAvailability, DecisionEvaluationRecord, DecisionMode, DecisionPreferences } from '../../types/domain';

const defaultPreferences: DecisionPreferences = { mode: 'off', modelId: 'jev-latest' };

export function JevDecisionSettings() {
  const [preferences, setPreferences] = useState(defaultPreferences);
  const [availability, setAvailability] = useState<DecisionAvailability>({ connected: false, models: [] });
  const [evaluations, setEvaluations] = useState<DecisionEvaluationRecord[]>([]);
  const [apiKey, setApiKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!desktopAvailable) { setLoading(false); return; }
    setLoading(true);
    setError(null);
    try {
      const [saved, status, baseline] = await Promise.all([
        command('get_jev_decision_preferences', undefined),
        command('get_jev_availability', undefined),
        command('get_jev_evaluations', undefined),
      ]);
      setPreferences(saved);
      setAvailability(status);
      setEvaluations(baseline);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Jev settings could not be loaded.');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  async function connect(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!apiKey.trim() || busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await command('connect_jev_api_key', { secret: apiKey });
      setApiKey('');
      setAvailability(result);
      const nextModel = result.models.some(model => model.id === preferences.modelId)
        ? preferences.modelId
        : result.models[0]?.id ?? preferences.modelId;
      const nextPreferences = { ...preferences, modelId: nextModel };
      if (nextModel !== preferences.modelId) {
        setPreferences(await command('save_jev_decision_preferences', { preferences: nextPreferences }));
      }
      setNotice('Jev is connected. Choose how it should support coding tasks below.');
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Jev could not connect. Check the API key and try again.');
    } finally {
      setBusy(false);
    }
  }

  async function save(next: DecisionPreferences) {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setPreferences(await command('save_jev_decision_preferences', { preferences: next }));
      setNotice('Decision assistance preferences saved.');
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Decision assistance preferences could not be saved.');
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setAvailability(await command('disconnect_jev', undefined));
      setApiKey('');
      setPreferences(current => ({ ...current, mode: 'off' }));
      setNotice('Jev was disconnected and decision assistance is off.');
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Jev could not be disconnected.');
    } finally {
      setBusy(false);
    }
  }

  async function changeMode(mode: DecisionMode) {
    await save({ ...preferences, mode });
  }

  async function changeModel(modelId: string) {
    await save({ ...preferences, modelId });
  }

  async function runEvaluation() {
    if (busy || !availability.connected) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await command('run_jev_evaluation', undefined);
      setEvaluations(current => [...result.slice().reverse(), ...current].slice(0, 100));
      const matches = result.filter(item => !item.failureCode && item.mismatchedFields.length === 0).length;
      const failed = result.filter(item => item.failureCode).length;
      setNotice(`${matches} of ${result.length} synthetic examples matched${failed ? `; ${failed} request${failed === 1 ? '' : 's'} failed` : ''}.`);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'The Jev evaluation could not be completed.');
    } finally {
      setBusy(false);
    }
  }

  const selectedModelAvailable = availability.models.some(model => model.id === preferences.modelId);
  const desktopMissing = !desktopAvailable;
  const latestEvaluationRunId = evaluations[0]?.runId;
  const latestEvaluationRun = latestEvaluationRunId
    ? evaluations.filter(item => item.runId === latestEvaluationRunId)
    : [];

  return <section className="settings-content jev-decision-settings" aria-labelledby="jev-decision-title">
    <div className="settings-heading-row">
      <div>
        <h1 id="jev-decision-title">Decision assistance</h1>
        <p className="page-description">Configure Jev by TypeSafe as a typed decision service alongside your coding model.</p>
      </div>
      <button className="quiet-icon-button" onClick={() => void refresh()} disabled={loading || busy || desktopMissing} aria-label="Refresh Jev status" title="Refresh status">
        {loading ? <LoaderCircle size={14} className="spin" /> : <RefreshCw size={14} />}
      </button>
    </div>

    <section className="jev-connection-card" aria-label="Jev connection">
      <div className="jev-connection-heading">
        <div className="jev-mark" aria-hidden="true">J</div>
        <div><h2>Jev by TypeSafe</h2><p>Typed choices and scores for focused agent decisions.</p></div>
        <span className={`jev-connection-state${availability.connected ? ' connected' : ''}`}>
          <span />{availability.connected ? 'Connected' : 'Not connected'}
        </span>
      </div>

      {desktopMissing ? <p className="jev-inline-state">Connect Jev from the desktop app. Secure credential storage is unavailable in browser preview.</p> : loading ?
        <div className="jev-inline-state" role="status"><LoaderCircle size={14} className="spin" /> Loading Jev status…</div> :
        availability.connected ? <div className="jev-connected-details">
          <div className="jev-safe-note"><ShieldCheck size={15} /><span>API key is stored in your OS credential store. It is never saved in SQLite or returned to the interface.</span></div>
          <div className="jev-connection-actions">
            <span>{availability.models.length} {availability.models.length === 1 ? 'model' : 'models'} available</span>
            <button className="account-button quiet" onClick={() => void refresh()} disabled={busy}><RefreshCw size={13} />Refresh models</button>
            <button className="account-button remove" onClick={() => void disconnect()} disabled={busy}><Trash2 size={13} />Disconnect</button>
          </div>
        </div> : <form className="account-connect-form jev-key-form" onSubmit={event => void connect(event)}>
          <label htmlFor="jev-api-key"><span>TypeSafe API key</span><input id="jev-api-key" type="password" autoComplete="off" value={apiKey} onChange={event => setApiKey(event.target.value)} placeholder="Paste API key" disabled={busy} /></label>
          <button type="submit" className="account-button connect" disabled={busy || !apiKey.trim()}>{busy ? <LoaderCircle size={13} className="spin" /> : <KeyRound size={13} />}{busy ? 'Testing…' : 'Test and connect'}</button>
        </form>}
    </section>

    {error && <div className="jev-settings-feedback error" role="alert"><CircleAlert size={14} /><span>{error}</span></div>}
    {notice && <div className="jev-settings-feedback success" role="status"><CircleCheck size={14} /><span>{notice}</span></div>}

    <section className="jev-preferences-section" aria-labelledby="jev-preferences-heading">
      <div className="permission-section-heading">
        <div><h2 id="jev-preferences-heading">How Jev should help</h2><p>When enabled, Jev receives the current request. Shadow mode also sends a bounded task-evidence summary as described below. File contents, diffs, terminal output, and raw command/search arguments are not sent.</p></div>
        <ShieldCheck size={17} aria-hidden="true" />
      </div>
      <label className="permission-mode-control" htmlFor="jev-mode-select">
        <span>Decision mode</span>
        <select id="jev-mode-select" value={preferences.mode} disabled={busy || loading || desktopMissing || !availability.connected} onChange={event => void changeMode(event.target.value as DecisionMode)}>
          <option value="off">Off</option>
          <option value="shadow">Shadow evaluation</option>
          <option value="advisory">Advisory</option>
          <option value="assisted">Assisted</option>
        </select>
      </label>
      <p className="permission-mode-description">
        {preferences.mode === 'off' ? 'Jev is not used in coding tasks.' : preferences.mode === 'shadow' ? 'Jev evaluates the request and records suggestions without steering the agent. Per task, up to one completion review (8 request items, 24 changed workspace-relative paths, and aggregate test outcomes), 3 scope checks (objective and candidate relative path), and 1 recovery check (objective and repeated tool category/count) may be sent to Jev. Stage 2 currently builds a local-only file, test, and available-tool baseline; filenames and tool names are not sent to Jev. Suggestions never change actions, approvals, permissions, or file state.' : preferences.mode === 'advisory' ? 'Jev may suggest a task category. The agent and permission system remain in control. Stage 1 recovery and scope checks are only sent in Shadow mode.' : 'Jev can rank context focus and the order of eligible tool definitions, and suggest a workflow. It never removes tools, runs actions, or changes permission checks. Stage 1 recovery and scope checks are only sent in Shadow mode.'}
      </p>
      <label className="permission-mode-control" htmlFor="jev-model-select">
        <span>Jev model</span>
        <select id="jev-model-select" value={preferences.modelId} disabled={!availability.connected || busy || loading || availability.models.length === 0} onChange={event => void changeModel(event.target.value)}>
          {!selectedModelAvailable && <option value={preferences.modelId}>Previously selected model unavailable</option>}
          {availability.models.map(model => <option key={model.id} value={model.id}>{model.id}</option>)}
        </select>
      </label>
      {availability.connected && !selectedModelAvailable && <p className="jev-model-warning">The saved model is no longer available. Select an available model before enabling Jev.</p>}
      <div className="permission-safety-note"><ShieldCheck size={15} /><span>Jev is a decision aid, not a coding model or security authority. Jev suggestions cannot approve file edits, terminal commands, network access, or dangerous actions.</span></div>
    </section>

    <section className="jev-evaluation-section" aria-labelledby="jev-evaluation-title">
      <div className="jev-evaluation-heading">
        <div><h2 id="jev-evaluation-title">Decision quality baseline</h2><p>Replay four fixed examples against the selected Jev model and compare its decisions with a small expected-answer set.</p></div>
        <button className="account-button quiet" type="button" onClick={() => void runEvaluation()} disabled={!availability.connected || !selectedModelAvailable || busy || loading || desktopMissing}>
          {busy ? <LoaderCircle size={13} className="spin" /> : <FlaskConical size={13} />}{busy ? 'Evaluating…' : 'Run evaluation'}
        </button>
      </div>
      <p className="jev-evaluation-privacy">This sends only four synthetic example requests and decision metadata. No active task text, project path, or repository files are sent. TypeSafe may count these requests toward Jev usage.</p>
      {latestEvaluationRun.length > 0 && <div className="jev-evaluation-results" aria-live="polite">
        <div className="jev-evaluation-summary">
          {latestEvaluationRun.filter(item => !item.failureCode && item.mismatchedFields.length === 0).length}/{latestEvaluationRun.length} matched · {latestEvaluationRun.reduce((total, item) => total + item.inputTokens + item.outputTokens, 0).toLocaleString()} tokens · suite {latestEvaluationRun[0].suiteVersion}
        </div>
        {latestEvaluationRun.map(item => {
          const passed = !item.failureCode && item.mismatchedFields.length === 0;
          const comparison = Object.entries(item.expectedAnswers)
            .map(([field, expected]) => `${field}: ${item.actualAnswers[field] ?? 'no answer'} (expected ${expected.join(' or ')})`)
            .join(' · ');
          return <article className={`jev-evaluation-result${passed ? ' is-match' : ' is-mismatch'}`} key={item.id}>
            <div><strong>{item.caseTitle}</strong><small>{item.resolvedModelId ?? item.requestedModelId}{item.failureCode ? ` · ${item.failureCode}` : ''}</small><small>{comparison}</small></div>
            <span>{item.failureCode ? 'Failed' : passed ? 'Matched' : `Review ${item.mismatchedFields.join(', ')}`}</span>
          </article>;
        })}
      </div>}
    </section>
  </section>;
}
