import { useEffect, useRef, useState } from 'react';
import { Check, Folder, LoaderCircle, RefreshCw, TriangleAlert } from 'lucide-react';
import { message, rpc, type AdoptionReport, type AdoptionScope } from './api';
import Modal from './Modal';
const labels: Record<string, string> = { pending: 'Ready to inventory', drive_inventory: 'Reading Google Drive', local_inventory: 'Checking local files', rechecking: 'Rechecking changes', matching: 'Matching existing content', ready: 'Report ready', cancelled: 'Inventory cancelled', failed: 'Inventory needs attention' };
const statuses: Record<string, string> = { matched: 'Already synchronized', unresolved: 'Needs review', upload: 'Proposed upload', download: 'Proposed download', protected: 'Protected items', excluded: 'Excluded' };
const formatBytes = (value: number) => value < 1024 * 1024 ? `${(value / 1024).toFixed(1)} KB` : value < 1024 ** 3 ? `${(value / 1024 ** 2).toFixed(1)} MB` : `${(value / 1024 ** 3).toFixed(1)} GB`;
export default function Adoption({ onActivated }: { onActivated: () => void }) {
  const [report, setReport] = useState<AdoptionReport>();
  const [name, setName] = useState(''); const [status, setStatus] = useState(''); const [offset, setOffset] = useState(0);
  const [error, setError] = useState(''); const [notice, setNotice] = useState(''); const [busy, setBusy] = useState(false);
  const [scope, setScope] = useState<AdoptionScope>(); const [acknowledged, setAcknowledged] = useState(false);
  const [excludes, setExcludes] = useState(''); const initialized = useRef(false);
  useEffect(() => {
    let cancelled = false; let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const result = await rpc<AdoptionReport>('adoption_report', { name, status: status || null, offset });
        if (!cancelled) { setReport(result); if (!initialized.current) { setExcludes(result.source.excludes.join('\n')); initialized.current = true; } }
      } catch (e) { if (!cancelled) setError(message(e)); }
      if (!cancelled) timer = setTimeout(poll, 2500);
    }
    void poll(); return () => { cancelled = true; clearTimeout(timer); };
  }, [name, status, offset]);
  const action = async (command: string, args: Record<string, unknown> = {}) => {
    setBusy(true); setError(''); setNotice('');
    try { return await rpc(command, args); } catch (e) { setError(message(e)); return undefined; } finally { setBusy(false); }
  };
  const review = async (path: string) => {
    const value = await action('adoption_scope', { scope: path, revision: report?.progress.revision });
    if (value) { setScope(value as AdoptionScope); setAcknowledged(false); }
  };
  const activate = async () => {
    if (!scope || !acknowledged) return;
    const value = await action('adoption_activate', { scope: scope.scope, revision: scope.revision });
    if (value) { setNotice(`Sync activated for ${scope.scope}. The profile and manifest were backed up.`); setScope(undefined); onActivated(); }
  };
  const running = !!report && ['drive_inventory', 'local_inventory', 'rechecking', 'matching'].includes(report.progress.phase);
  return <section className="adoption" aria-label="Existing folder adoption">
    <div className="section-heading"><h2>Adopt existing files</h2><button onClick={() => { void action('open_adoption'); }}><Folder />Open report folder</button></div>
    <p>Compare your existing local tree with My Drive. This inventory reads metadata and file checksums; it makes no uploads, downloads or deletions.</p>
    {error && <div role="alert" className="banner error"><TriangleAlert /><span>{error}</span></div>}
    {notice && <div role="status" className="banner"><Check /><span>{notice}</span></div>}
    {!report ? <p className="loading"><LoaderCircle className="spin" />Loading adoption report…</p> : <>
      <div className="path-columns"><div><label>Existing local root</label><p>{report.source.local_root}</p></div><div><label>Google Drive account</label><p>{report.source.account_email}</p><span className="muted">Drive root ID {report.source.remote_root_id}</span></div></div>
      <div className="adoption-progress"><strong>{labels[report.progress.phase] ?? report.progress.phase}</strong>{running && <LoaderCircle className="spin" />}<span>{report.progress.drive_passes ? `Pass ${report.progress.drive_passes} · ` : ''}{report.progress.remote_items.toLocaleString()} Drive items · {report.progress.local_items.toLocaleString()} local items · {formatBytes(report.progress.hashed_bytes)} hashed · {report.progress.reused_hashes.toLocaleString()} saved checksums reused</span>{report.progress.current_path && <span className="work-path">{report.progress.current_path}</span>}{report.progress.completed_at && <span>Compared {new Date(report.progress.completed_at * 1000).toLocaleString()}</span>}</div>
      {report.progress.error && <p role="alert" className="inline-error">{report.progress.error.message}</p>}
      <details><summary>Review exclusions ({report.source.excludes.length})</summary><p>{report.source.exclusion_source}</p><p>Already managed folders and the active FreeSync development workspace remain outside this report.</p>{report.progress.phase === 'pending' ? <label>Relative path globs, one per line<textarea rows={5} value={excludes} onChange={e => setExcludes(e.target.value)} /></label> : <ul>{report.source.excludes.map(rule => <li key={rule}><code>{rule}</code></li>)}</ul>}</details>
      <div className="adoption-actions">{running ? <button disabled={busy} onClick={() => { void action('adoption_cancel'); }}>Cancel inventory</button> : <button disabled={busy} onClick={() => { void action('adoption_start', { excludes: excludes.split('\n').map(p => p.trim()).filter(Boolean) }); }}><RefreshCw />{report.progress.phase === 'pending' ? 'Start read-only inventory' : report.progress.phase === 'ready' ? 'Refresh report' : 'Resume inventory'}</button>}<span className="muted">Progress is saved outside the sync tree. Resume rechecks changed files.</span></div>
      <div className="adoption-counts">{Object.entries(statuses).map(([key, label]) => <button key={key} className={status === key ? 'selected' : ''} onClick={() => { setStatus(status === key ? '' : key); setOffset(0); }}><strong>{(report.counts[key] ?? 0).toLocaleString()}</strong><span>{label}</span></button>)}</div>
      {report.progress.phase !== 'ready' && <p className="muted">Activation is unavailable until inventory and consistency checks finish. Any previous findings below are provisional.</p>}
      <div className="adoption-filters"><label>Filter by file or folder name<input placeholder="Search relative paths" value={name} onChange={e => { setName(e.target.value); setOffset(0); }} /></label><label>Result<select value={status} onChange={e => { setStatus(e.target.value); setOffset(0); }}><option value="">All results</option>{Object.entries(statuses).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></label></div>
      <div className="adoption-findings">{report.findings.map(item => <article key={item.path}><div><strong>{item.path || 'Root'}</strong><p>{item.reason}</p><span className="muted">{statuses[item.status]}{item.size_bytes !== null && item.size_bytes !== undefined ? ` · ${formatBytes(item.size_bytes)}` : ''}</span></div>{item.kind === 'folder' && item.status === 'matched' && <button disabled={busy || report.progress.phase !== 'ready'} onClick={() => { void review(item.path); }}>Review folder</button>}</article>)}{report.progress.phase === 'ready' && !report.findings.length && <p>No matching results.</p>}</div>
      <div className="adoption-actions"><button disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 50))}>Previous</button><span>{report.matching ? `${offset + 1}–${Math.min(offset + 50, report.matching)} of ${report.matching.toLocaleString()}` : 'No findings yet'}</span><button disabled={offset + 50 >= report.matching} onClick={() => setOffset(offset + 50)}>Next</button></div>
    </>}
    {scope && <Modal title={`Activate ${scope.scope}`} onClose={() => { if (!busy) setScope(undefined); }}><p><strong>Only this folder and its included descendants will sync.</strong> Other folders remain outside this activation.</p><dl className="detail-rows"><div><dt>Local folder</dt><dd>{scope.pair.local_root}</dd></div><div><dt>Drive folder</dt><dd>{scope.pair.remote_root_name} · {scope.pair.remote_root_id}</dd></div><div><dt>Account</dt><dd>{scope.pair.account_email}</dd></div>{Object.entries(scope.counts).map(([key, count]) => <div key={key}><dt>{statuses[key] ?? key}</dt><dd>{count.toLocaleString()}</dd></div>)}</dl>{scope.pair.excludes.length > 0 && <details><summary>Excluded or protected paths ({scope.pair.excludes.length})</summary><ul>{scope.pair.excludes.map(rule => <li key={rule}><code>{rule}</code></li>)}</ul></details>}<p>Equivalent files keep their existing Drive IDs. Proposed uploads and downloads begin after activation; initial differences remain conflicts. FreeSync rechecks the reviewed versions and backs up its configuration and manifest first.</p><label className="adoption-consent"><input type="checkbox" checked={acknowledged} onChange={e => setAcknowledged(e.target.checked)} />I reviewed this scope and authorize two-way synchronization for this folder.</label><div className="adoption-actions"><button className="primary" disabled={!acknowledged || busy || report?.progress.phase !== 'ready' || report.progress.revision !== scope.revision} onClick={() => { void activate(); }}>{busy ? <LoaderCircle className="spin" /> : <Check />}Activate reviewed folder</button><button disabled={busy} onClick={() => setScope(undefined)}>Close review</button></div></Modal>}
    <details className="adoption-rollback"><summary>Return to InSync</summary><p>Pause FreeSync from the toolbar or tray, then quit it from the tray before starting InSync again. Keep the local and Drive trees in place. Disable FreeSync’s start-at-login option if you plan to keep using InSync.</p><p>Pre-activation backups live in the profile’s backups folder. Keep the adoption manifest and profile backup; restoring a profile is a stopped-app operation. Switching clients does not erase or automatically undo files already synchronized.</p></details>
  </section>;
}
