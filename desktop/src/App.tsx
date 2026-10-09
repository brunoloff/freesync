import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowLeftRight, ArrowRight, Check, ChevronRight, Eye, History, FileDiff, Folder, LoaderCircle, Minus, Pause, Play, Plus, RefreshCw, Settings, X, TriangleAlert } from 'lucide-react';
import { message, native, rpc, type Conflict, type ConflictChoice, type ConflictJob, type FolderPage, type Pair, type Preview, type Snapshot } from './api';
import Activity from './Activity';
import Adoption from './Adoption';
import Modal from './Modal';
import { invoke } from '@tauri-apps/api/core';
type Page = 'Folders' | 'Conflicts' | 'Activity' | 'Adoption' | 'Preferences';
const compactPath = (path: string) => path.replace(/^\/home\/[^/]+\//, '~/').replace(/^\/Users\/[^/]+\//, '~/');
const bytes = (value?: number) => value === undefined ? 'Unavailable' : value < 1024 ? `${value} B` : value < 1024 * 1024 ? `${(value / 1024).toFixed(1)} KB` : `${(value / 1024 / 1024).toFixed(1)} MB`;
function statusLabel(pair: Pair, paused: boolean): string {
  if (!pair.enabled) return 'Ready to activate';
  if (paused) return 'Paused';
  if (pair.status.error || pair.deletion_hold) return 'Needs attention';
  if (pair.status.conflicts) return `${pair.status.conflicts} conflict${pair.status.conflicts === 1 ? '' : 's'}`;
  return ({ idle: 'Up to date', scanning: 'Checking changes', syncing: 'Syncing', waiting_retry: 'Retry scheduled', reconnect: 'Reconnect account', offline: 'Waiting for network', stopped: 'Stopped' } as Record<string, string>)[pair.status.state] ?? 'Waiting';
}
function StatusMark({ good = false }: { good?: boolean }) { return good ? <span className="check-mark"><Check size={14} /></span> : <span className="status-dot" />; }
export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [page, setPage] = useState<Page>('Folders');
  const [selected, setSelected] = useState('test-freesync');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [pairDialog, setPairDialog] = useState(false);
  const [login, setLogin] = useState(false);
  const [preview, setPreview] = useState<{ pairId: string; result: Preview; wasPaused: boolean }>();
  const [pendingConflicts, setPendingConflicts] = useState<Set<string>>(() => new Set());
  const pendingConflictIds = useRef(new Set<string>());
  const [conflictErrors, setConflictErrors] = useState<Record<string, string>>({});
  const [zoom, setZoom] = useState(1);
  const zoomLevel = useRef(1);
  const zoomPending = useRef(false);
  const zoomTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const reportedNativeView = useRef<number | undefined>(undefined);
  const previousJobs = useRef<Record<string, string>>({});
  const refresh = useCallback(async () => { const value = await rpc<Snapshot>('snapshot'); setSnapshot(value); }, []);
  const changeZoom = useCallback((level: number) => {
    const next = Math.round(Math.min(2, Math.max(.5, level)) * 100) / 100;
    zoomLevel.current = next; setZoom(next); zoomPending.current = true;
    clearTimeout(zoomTimer.current);
    zoomTimer.current = setTimeout(() => {
      void rpc('zoom', { level: next }).catch(e => setError(message(e))).finally(() => { if (zoomLevel.current === next) zoomPending.current = false; });
    }, 100);
  }, []);
  useEffect(() => {
    const saved = snapshot?.preferences.zoom;
    if (saved && !zoomPending.current) { zoomLevel.current = saved; setZoom(saved); }
  }, [snapshot?.preferences.zoom]);
  useEffect(() => {
    if (!native) document.documentElement.style.zoom = String(zoom);
  }, [zoom]);
  useEffect(() => {
    const wheel = (event: WheelEvent) => { if (event.ctrlKey && event.deltaY) { event.preventDefault(); changeZoom(zoomLevel.current + (event.deltaY < 0 ? .1 : -.1)); } };
    const key = (event: KeyboardEvent) => { if ((event.ctrlKey || event.metaKey) && ['+', '=', '-', '0'].includes(event.key)) { event.preventDefault(); changeZoom(event.key === '0' ? 1 : zoomLevel.current + (event.key === '-' ? -.1 : .1)); } };
    document.addEventListener('wheel', wheel, { passive: false, capture: true }); document.addEventListener('keydown', key);
    return () => { clearTimeout(zoomTimer.current); document.removeEventListener('wheel', wheel, true); document.removeEventListener('keydown', key); };
  }, [changeZoom]);
  useEffect(() => {
    for (const job of snapshot?.conflict_jobs ?? []) {
      if (job.state === 'done' && previousJobs.current[job.id] && previousJobs.current[job.id] !== 'done') setNotice(job.choice === 'compare' ? 'The text comparison opened in your system diff app. Its temporary copies are read-only.' : job.choice === 'refresh' ? 'Fresh comparison complete.' : 'Conflict resolved. The displaced version remains preserved.');
      previousJobs.current[job.id] = job.state;
    }
  }, [snapshot?.conflict_jobs]);
  useEffect(() => {
    if (import.meta.env.VITE_FREESYNC_NATIVE_PROBE !== '1' || !native || !snapshot || reportedNativeView.current === snapshot.preferences.zoom) return;
    reportedNativeView.current = snapshot.preferences.zoom;
    const heading = document.querySelector('main h1');
    void invoke('native_ui_probe', { report: {
      mounted: !!document.querySelector('.app-shell') && heading?.textContent === 'Folders',
      pair_count: document.querySelectorAll('.pair-row').length,
      account_connected: document.querySelector('.account-status span:not(.status-dot)')?.textContent === 'Connected',
      pause_available: [...document.querySelectorAll('button')].some(button => button.textContent === 'Pause' && !button.disabled),
      width: innerWidth, height: innerHeight,
      document_width: document.documentElement.scrollWidth,
      heading_pixels: heading ? parseFloat(getComputedStyle(heading).fontSize) : 0,
    } }).catch(() => {});
  }, [snapshot]);
  useEffect(() => {
    let cancelled = false; let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try { const value = await rpc<Snapshot>('snapshot'); if (!cancelled) setSnapshot(value); }
      catch (e) { if (!cancelled) setError(message(e)); }
      if (!cancelled) timer = setTimeout(poll, 2000);
    }
    void poll();
    void rpc('verify_account').then(() => { if (!cancelled) void refresh(); }).catch(e => { if (!cancelled) setError(message(e)); });
    return () => { cancelled = true; clearTimeout(timer); };
  }, [refresh]);
  const action = async (command: string, args: Record<string, unknown> = {}) => {
    setBusy(true); setError(''); setNotice('');
    try { const value = await rpc(command, args); await refresh(); return value; }
    catch (e) { setError(message(e)); throw e; }
    finally { setBusy(false); }
  };
  const control = (name: string) => { void action('control', { action: name }).catch(() => {}); };
  const pair = snapshot?.pairs.find(p => p.id === selected) ?? snapshot?.pairs[0];
  const paused = snapshot?.controls.paused ?? false;
  const prepare = async (pairId: string) => {
    const wasPaused = paused;
    const result = await action('preview', { pairId }) as Preview;
    setPreview({ pairId, result, wasPaused });
  };
  const closePreview = () => {
    const previous = preview; setPreview(undefined);
    if (previous && !previous.wasPaused && snapshot?.pairs.find(p => p.id === previous.pairId)?.enabled) control('resume');
  };
  const resolve = async (conflict: Conflict, choice: ConflictChoice) => {
    if (pendingConflictIds.current.has(conflict.id)) return;
    pendingConflictIds.current.add(conflict.id); setPendingConflicts(new Set(pendingConflictIds.current));
    setConflictErrors(previous => ({ ...previous, [conflict.id]: '' }));
    try {
      await rpc('resolve_conflict', { pairId: conflict.pair_id, path: conflict.path, conflictId: conflict.id, choice });
      setNotice('Action queued in the background. You can review other conflicts.'); await refresh();
    } catch (e) { setConflictErrors(previous => ({ ...previous, [conflict.id]: message(e) })); }
    finally { pendingConflictIds.current.delete(conflict.id); setPendingConflicts(new Set(pendingConflictIds.current)); }
  };
  const good = !!pair?.enabled && !paused && !pair.status.error && !pair.status.conflicts && !pair.deletion_hold;
  const footer = paused ? 'Sync is paused' : snapshot?.pairs.some(p => p.enabled) ? 'Sync is running' : 'Choose a folder to start syncing';
  return <div className="app-shell" data-window-visible={snapshot?.window_visible} data-tray-available={snapshot?.tray_available} data-native-view-ready={snapshot?.native_ui?.mounted} data-native-ipc-verified={snapshot?.native_ui?.ipc_verified}>
    <aside className="sidebar"><div className="wordmark">FreeSync</div><nav aria-label="Main navigation">{(['Folders', 'Conflicts', 'Activity', 'Adoption'] as Page[]).map(name => {
      const Icon = name === 'Folders' || name === 'Adoption' ? Folder : name === 'Conflicts' ? ArrowLeftRight : History;
      return <button key={name} className={page === name ? 'nav-link selected' : 'nav-link'} aria-current={page === name ? 'page' : undefined} onClick={() => setPage(name)}><Icon />{name}{name === 'Conflicts' && !!snapshot?.conflicts.length && <span className="count">{snapshot.conflicts.length}</span>}</button>;
    })}</nav><div className="sidebar-bottom"><div className="account-status"><span className="status-dot" /><div><strong>Personal Google account</strong><span>{snapshot?.account_verified ? 'Connected' : snapshot?.account ? 'Checking connection' : 'Not connected'}</span></div></div><div className="sidebar-actions"><button className={page === 'Preferences' ? 'nav-link selected' : 'nav-link'} aria-current={page === 'Preferences' ? 'page' : undefined} onClick={() => setPage('Preferences')}><Settings />Preferences</button></div></div></aside>
    <div className="main-shell"><main><header className="page-heading"><div><h1>{page}</h1><p>{page === 'Folders' ? 'Choose what stays in sync.' : page === 'Conflicts' ? 'Preserve both versions when changes overlap.' : page === 'Activity' ? 'See what happened and investigate sync problems.' : page === 'Adoption' ? 'Reuse your existing synchronized files.' : 'Make FreeSync work the way you want.'}</p></div><div className="heading-actions"><button disabled={busy} onClick={() => control(paused ? 'resume' : 'pause')}>{paused ? <Play /> : <Pause />}{paused ? 'Resume' : 'Pause'}</button><button disabled={busy} onClick={() => control('sync_now')}><RefreshCw />Sync now</button></div></header>
      {error && <div role="alert" className="banner error"><TriangleAlert /><span>{error}</span><button aria-label="Dismiss error" className="icon-button" onClick={() => setError('')}><X /></button></div>}
      {snapshot && !snapshot.window_visible && <div role="status" className="banner"><Eye /><span>Settings are hidden. Sync continues in the tray.</span><button onClick={() => { void action('show').catch(() => {}); }}>Show settings</button></div>}
      {notice && <div role="status" className="banner"><Check /><span>{notice}</span><button aria-label="Dismiss message" className="icon-button" onClick={() => setNotice('')}><X /></button></div>}
      {!snapshot ? <div className="loading"><LoaderCircle className="spin" />Connecting to the background engine…</div> : page === 'Adoption' ? <Adoption onActivated={() => { void refresh(); }} /> : page === 'Folders' ? <>
        <div className="section-heading"><h2>Your folders</h2><button className="primary" disabled={busy} onClick={() => setPairDialog(true)}><Plus />Add folder</button></div>
        <div className="pair-list">{snapshot.pairs.map(p => <button key={p.id} className={`pair-row ${pair?.id === p.id ? 'selected' : ''}`} onClick={() => setSelected(p.id)} aria-pressed={pair?.id === p.id}><Folder size={27} /><div className="pair-title"><strong>{p.remote_name}</strong><span>Two-way sync</span></div><div className="pair-status"><StatusMark good={p.enabled && !paused && !p.status.conflicts && !p.status.error} />{statusLabel(p, paused)}</div><ChevronRight /></button>)}</div>
        {pair ? <section className="pair-details" aria-label="Selected folder details"><div className="path-columns"><div><label>Local folder</label><p>{compactPath(pair.local_root)}</p></div><div><label>Google Drive folder</label><p>My Drive / {pair.remote_path ?? pair.remote_name}</p></div></div><dl className="detail-rows"><div><dt>Last synced</dt><dd>{pair.status.last_sync ? new Date(pair.status.last_sync * 1000).toLocaleString() : 'Not yet'}</dd></div><div><dt>Pending changes</dt><dd>{pair.status.queued || 'None'}</dd></div>{pair.status.current_path && <div><dt>Current work</dt><dd className="work-path">{pair.status.current_path}{pair.status.total_bytes > 0 && <><progress value={pair.status.progress_bytes} max={pair.status.total_bytes} />{bytes(pair.status.progress_bytes)} of {bytes(pair.status.total_bytes)}</>}</dd></div>}{pair.status.retry_at && <div><dt>Next retry</dt><dd>{new Date(pair.status.retry_at * 1000).toLocaleTimeString()}</dd></div>}</dl>
          {pair.status.error && <div role="alert" className="banner error"><TriangleAlert /><span>{pair.status.error.message}</span>{pair.status.state === 'reconnect' && <button onClick={() => setLogin(true)}>Reconnect</button>}</div>}
          {pair.deletion_hold && <div className="banner error"><TriangleAlert /><span>{pair.deletion_count} deletions are paused for review. Review their paths in the preview before continuing.</span><button disabled={busy} onClick={() => { void prepare(pair.id).catch(() => {}); }}>Review</button></div>}
          <div className="detail-actions"><button disabled={busy} onClick={() => { void prepare(pair.id).catch(() => {}); }}>Preview changes{busy && <LoaderCircle className="spin" />}</button><button className="text-button" disabled={busy} onClick={() => setPairDialog(true)}>Folder settings</button></div><p className="muted recovery-note">Deleted files are kept in recovery.</p>
        </section> : <div className="empty"><Folder /><h3>No folders yet</h3><p>Add the test folder, or use Adoption to review existing synchronized folders.</p><button className="primary" onClick={() => setPairDialog(true)}>Add folder</button></div>}
      </> : page === 'Conflicts' ? <section className="conflict-list">
        {snapshot.conflicts.length ? snapshot.conflicts.map(c => <ConflictRow key={c.id} conflict={c} job={snapshot.conflict_jobs.find(j => j.conflict_id === c.id)} pending={pendingConflicts.has(c.id)} error={conflictErrors[c.id]} onAction={choice => { void resolve(c, choice); }} />) : <div className="empty"><Check /><h3>No conflicts</h3><p>Your folder pairs have no unresolved conflicts.</p></div>}
        {snapshot.conflict_jobs.some(j => !['done', 'failed'].includes(j.state)) && <section className="background-actions" aria-label="Background conflict actions"><h2>Background actions</h2>{snapshot.conflict_jobs.filter(j => !['done', 'failed'].includes(j.state)).map(j => <div key={j.id} role="status"><LoaderCircle className="spin" /><div><strong>{j.path}</strong><span>{j.state === 'syncing' && paused ? 'Waiting for sync to resume' : jobLabel(j)}</span></div></div>)}</section>}
      </section> : page === 'Activity' ? <Activity /> : <section className="preferences">
        <div className="preference-row"><div><h2>Google account</h2><p>{snapshot.account ?? 'Connect an account to browse Drive.'}</p></div><button disabled={busy} onClick={() => setLogin(true)}>{snapshot.account ? 'Reconnect' : 'Connect Google account'}</button></div>
        <div className="preference-row"><div><h2>Start FreeSync at login</h2><p>Keep syncing in the tray when you sign in to this computer.</p></div><input aria-label="Start FreeSync at login" type="checkbox" className="switch" checked={snapshot.autostart} disabled={busy} onChange={e => { void action('preferences', { ...snapshot.preferences, autostart: e.target.checked }).catch(() => {}); }} /></div>
        <div className="preference-row"><div><h2>Notifications</h2><p>Notify you when sync needs attention.</p><button className="text-button" disabled={busy} onClick={() => { void action('notify_test').then(result => { const delivery = result as { inhibited?: boolean; accepted?: boolean }; setNotice(delivery.inhibited ? 'Your desktop is currently suppressing notifications (Do Not Disturb or presentation mode). The test was accepted, but its popup may be hidden.' : delivery.accepted ? 'Your desktop accepted the test notification. If its popup is hidden, check the system notification settings.' : 'Test notification requested. Check the system notification settings if no popup appears.'); }).catch(() => {}); }}>Test notification</button></div><input aria-label="Notifications" type="checkbox" className="switch" checked={snapshot.preferences.notifications} disabled={busy} onChange={e => { void action('preferences', { notifications: e.target.checked }).catch(() => {}); }} /></div>
        <div className="preference-row"><div><h2>Interface zoom</h2><p>Ctrl + scroll zooms the entire interface. Ctrl + 0 resets it.</p></div><div className="zoom-controls"><button aria-label="Zoom out" disabled={zoom <= .5} onClick={() => changeZoom(zoom - .1)}><Minus /></button><button aria-label="Reset zoom" onClick={() => changeZoom(1)}>{Math.round(zoom * 100)}%</button><button aria-label="Zoom in" disabled={zoom >= 2} onClick={() => changeZoom(zoom + .1)}><Plus /></button></div></div>
        <div className="preference-row"><div><h2>Recovery files</h2><p className="path-value">{compactPath(snapshot.recovery_directory)}</p><p>Deleted and replaced files remain here until you remove them yourself.</p></div><button disabled={busy} onClick={() => { void action('open_recovery').then(() => setNotice('Recovery folder opened in your file manager.')).catch(() => {}); }}><Folder />Open folder</button></div>
        <div className="preference-row"><div><h2>Desktop OAuth client</h2><p>Use a Desktop client from your Google Cloud project.</p></div><button disabled={busy} onClick={() => { void action('import_client').then(value => { if ((value as { imported?: boolean }).imported) setNotice('Desktop client imported. You can connect your account.'); }).catch(() => {}); }}>Import client JSON</button></div>
      </section>}
    </main><footer className="status-footer"><span><StatusMark good={good} />{footer}</span><span>Folder settings are saved automatically</span></footer></div>
    {pairDialog && <PairDialog pair={pair} snapshot={snapshot} onClose={() => setPairDialog(false)} onReconnect={() => setLogin(true)} onSubmit={async (values) => { await action('configure', values); setPairDialog(false); await prepare('test-freesync'); }} />}
    {preview && <Modal title="Preview changes" description="Sync is paused while you review this plan." onClose={closePreview} footer={<><button disabled={busy} onClick={closePreview}>Cancel</button>{pair?.deletion_hold && <button disabled={busy} onClick={() => { void action('approve_deletions', { pairId: preview.pairId, count: pair.deletion_count }).catch(() => {}); }}>Approve {pair.deletion_count} deletions</button>}<button className="primary" disabled={busy || pair?.deletion_hold} onClick={() => { void action('activate', { pairId: preview.pairId }).then(() => { setPreview(undefined); setNotice('Sync is active for the selected folder pair.'); }).catch(() => {}); }}>{snapshot?.pairs.find(p => p.id === preview.pairId)?.enabled ? 'Resume sync' : 'Activate sync'}<ArrowRight /></button></>}><div className="preview-counts"><span><strong>{preview.result.counts.operations}</strong> changes</span><span><strong>{preview.result.counts.conflicts}</strong> conflicts</span><span><strong>{preview.result.counts.skipped}</strong> skipped</span></div><div className="preview-items">{preview.result.operations.map((o, i) => <div key={i}><strong>{o.path}</strong><span>{o.reason}</span></div>)}{preview.result.conflicts.map(c => <div key={c.path}><strong>{c.path}</strong><span>{c.reason}</span></div>)}{preview.result.skipped.map(s => <div key={s.path}><strong>{s.path}</strong><span>{s.reason}</span></div>)}{!preview.result.counts.operations && !preview.result.counts.conflicts && <p>No pending changes. Matching content is already in sync.</p>}</div><p className="muted">Uploads and downloads stay within the selected test folder. Deletions use Drive Trash or local recovery.</p></Modal>}
    {login && <LoginDialog account={snapshot?.account ?? ''} onClose={() => setLogin(false)} onSuccess={() => { setLogin(false); setError(''); setNotice('Google account connected.'); void refresh(); }} />}
  </div>;
}
function jobLabel(job: ConflictJob): string {
  return job.state === 'queued' ? 'Queued…' : job.state === 'ready_to_open' ? 'Opening diff app…' : job.state === 'syncing' ? 'Syncing the chosen versions…' : job.choice === 'compare' ? 'Preparing text comparison…' : 'Working…';
}
function ConflictRow({ conflict: c, job, pending, error, onAction }: { conflict: Conflict; job?: ConflictJob; pending: boolean; error?: string; onAction: (choice: ConflictChoice) => void }) {
  const working = pending || !!job && !['done', 'failed'].includes(job.state);
  return <article className="conflict-row" aria-label={`Conflict ${c.path}`}>
    <ArrowLeftRight /><div><h2>{c.path}</h2><p>{c.reason}</p><p className="muted">Local: {bytes(c.local_bytes)} · Google Drive: {bytes(c.remote_bytes)}</p>
      {working && <p role="status" className="conflict-progress"><LoaderCircle className="spin" />{job ? jobLabel(job) : 'Queuing…'}</p>}
      {(error || job?.error) && <p role="alert" className="inline-error">{error || job?.error?.message}</p>}
      {!c.can_keep_both && <p className="muted">Refresh missing, moved or ambiguous items before choosing a file version.</p>}
    </div><div className="conflict-controls">
      <button disabled={working || !c.can_keep_both} onClick={() => onAction('keep_both')}>Keep both</button>
      <button disabled={working || !c.can_use_local} title="Keep local content at the original name; retain the Drive original in recovery." onClick={() => onAction('use_local')}>Use local</button>
      <button disabled={working || !c.can_use_drive} title="Keep Drive content at the original name; retain the local original in recovery." onClick={() => onAction('use_drive')}>Use Drive</button>
      <button disabled={working || !c.can_use_drive} title="Open read-only temporary text copies in your system diff app." onClick={() => onAction('compare')}><FileDiff />Diff</button>
      <button className="text-button" disabled={working} onClick={() => onAction('refresh')}><RefreshCw />Refresh comparison</button>
    </div>
  </article>;
}
function PairDialog({ pair, snapshot, onClose, onReconnect, onSubmit }: { pair?: Pair; snapshot?: Snapshot; onClose: () => void; onReconnect: () => void; onSubmit: (values: Record<string, unknown>) => Promise<void> }) {
  const [path, setPath] = useState(pair ? compactPath(pair.local_root) : '');
  const [remoteId, setRemoteId] = useState(pair?.remote_id ?? '');
  const [remoteName, setRemoteName] = useState(pair?.remote_name ?? '');
  const [poll, setPoll] = useState(pair?.poll_secs ?? 10);
  const [limit, setLimit] = useState(pair?.deletion_limit ?? 20);
  const [folders, setFolders] = useState<FolderPage>();
  const [trail, setTrail] = useState([{ id: 'root', name: 'My Drive' }]);
  const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  const load = async (parent: string, page?: string) => {
    setBusy(true); setError('');
    try { const result = await rpc<FolderPage>('browse', { parent, page }); setFolders(previous => page && previous ? { ...result, folders: [...previous.folders, ...result.folders] } : result); }
    catch (e) { setError(message(e)); }
    finally { setBusy(false); }
  };
  useEffect(() => { void load('root'); }, []);
  const current = trail[trail.length - 1];
  const submit = async () => {
    setBusy(true); setError('');
    try { await onSubmit({ localPath: path, remoteId, pollSecs: poll, deletionLimit: limit }); }
    catch (e) { setError(message(e)); setBusy(false); }
  };
  return <Modal title="Choose a folder pair" description="Preview the changes before you turn on sync." onClose={onClose} footer={<><button disabled={busy} onClick={onClose}>Cancel</button><button className="primary" disabled={busy || !remoteId || !path || !Number.isInteger(limit) || limit < 1} onClick={() => { void submit(); }}>Preview changes{busy ? <LoaderCircle className="spin" /> : <ArrowRight />}</button></>}>
    {error && <p role="alert" className="inline-error">{error}</p>}<label>Google account</label><div className="account-picker"><span><StatusMark good={snapshot?.account_verified} />{snapshot?.account ?? 'Personal Google account'}</span><button onClick={onReconnect}>Reconnect</button></div>
    <label htmlFor="local-folder">Local folder</label><div className="input-with-button"><input id="local-folder" value={path} onChange={e => setPath(e.target.value)} spellCheck={false} /><button onClick={() => { void rpc<{ path?: string }>('pick_folder').then(value => { if (value.path) setPath(compactPath(value.path)); }).catch(e => setError(message(e))); }}>Browse</button></div>
    <label>Google Drive folder</label><div className="breadcrumbs">{trail.map((folder, i) => <span key={folder.id}><button className="text-button" disabled={busy} onClick={() => { setTrail(trail.slice(0, i + 1)); void load(folder.id); }}>{folder.name}</button>{i < trail.length - 1 && <ChevronRight size={16} />}</span>)}</div>
    <div className="folder-browser" aria-label="Google Drive folders">{busy && !folders ? <div className="loading"><LoaderCircle className="spin" />Loading folders…</div> : folders?.folders.map(folder => <div key={folder.id} className={`browser-row ${remoteId === folder.id ? 'selected' : ''}`}><button className="folder-select" onClick={() => { setRemoteId(folder.id); setRemoteName(folder.name); }} aria-pressed={remoteId === folder.id}><Folder />{folder.name}{remoteId === folder.id && <StatusMark good />}</button><button className="icon-button" aria-label={`Open ${folder.name}`} disabled={busy} onClick={() => { setTrail([...trail, { id: folder.id, name: folder.name }]); void load(folder.id); }}><ChevronRight /></button></div>)}{folders && !folders.folders.length && <p className="muted">No subfolders here.</p>}{folders?.next && <button className="text-button load-more" disabled={busy} onClick={() => { void load(current.id, folders.next); }}>Load more folders</button>}</div>
    <div className="browser-selection"><p className="muted">Selected: {remoteName ? `My Drive / ${remoteName}` : 'Choose a folder'}</p>{trail.length > 1 && <button className="text-button" onClick={() => { setRemoteId(current.id); setRemoteName(current.name); }}>Use this folder</button>}</div>
    <div className="form-columns"><div><label htmlFor="poll">Check Drive every</label><select id="poll" value={poll} onChange={e => setPoll(Number(e.target.value))}><option value={10}>10 seconds</option><option value={30}>30 seconds</option><option value={60}>1 minute</option><option value={300}>5 minutes</option></select></div><div><label htmlFor="deletion-limit">Pause before deleting more than</label><div className="number-input"><input id="deletion-limit" type="number" min={1} max={100000} value={limit} onChange={e => setLimit(Number(e.target.value))} /><span>files</span></div></div></div><p className="muted scope-note">This preview syncs only the test-freesync folder.</p>
  </Modal>;
}
function LoginDialog({ account, onClose, onSuccess }: { account: string; onClose: () => void; onSuccess: () => void }) {
  const [email, setEmail] = useState(account); const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [url, setUrl] = useState('');
  const mounted = useRef(true); useEffect(() => () => { mounted.current = false; }, []);
  const cancel = () => { void rpc('cancel_login').finally(onClose); };
  const connect = async () => {
    setBusy(true); setError('');
    try {
      const result = await rpc<{ authorization_url: string }>('begin_login', { account: email });
      if (!native) setUrl(result.authorization_url);
      await rpc('finish_login');
      if (mounted.current) onSuccess();
    } catch (e) { if (mounted.current) { setError(message(e)); setBusy(false); setUrl(''); } }
  };
  return <Modal title="Connect Google Drive" description="FreeSync connects directly to your Google account." onClose={cancel} footer={<><button onClick={cancel}>Cancel</button><button className="primary" disabled={busy || !email.includes('@')} onClick={() => { void connect(); }}>{busy ? 'Waiting for Google…' : 'Sign in with Google'}{busy && <LoaderCircle className="spin" />}</button></>}><label htmlFor="account-email">Google account email</label><input id="account-email" type="email" value={email} disabled={busy} onChange={e => setEmail(e.target.value)} /><p className="muted">Google will ask for access to read and update your Drive files. The preview applies changes only inside test-freesync. Credentials are saved in your operating system's credential store.</p>{url && <a href={url} target="_blank" rel="noreferrer" className="button-link">Open sign-in page<ArrowRight /></a>}{error && <p role="alert" className="inline-error">{error}</p>}</Modal>;
}
