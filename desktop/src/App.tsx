import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { ArrowLeftRight, ArrowRight, Check, ChevronRight, Eye, Folder, LoaderCircle, Pause, Play, Plus, Power, RefreshCw, Settings, X, TriangleAlert } from 'lucide-react';
import { message, native, rpc, type Conflict, type FolderPage, type Pair, type Preview, type Snapshot } from './api';
type Page = 'Folders' | 'Conflicts' | 'Preferences';
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
function Modal({ title, description, children, footer, onClose }: { title: string; description?: string; children: ReactNode; footer?: ReactNode; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const close = useRef(onClose); close.current = onClose;
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    const cancel = (event: Event) => { event.preventDefault(); close.current(); };
    const trap = (event: KeyboardEvent) => {
      if (event.key !== 'Tab') return;
      const controls = [...element.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])')].filter(control => control.getClientRects().length > 0);
      const first = controls[0]; const last = controls[controls.length - 1];
      if (!first) { event.preventDefault(); return; }
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    element.addEventListener('cancel', cancel);
    element.addEventListener('keydown', trap);
    return () => { element.removeEventListener('cancel', cancel); element.removeEventListener('keydown', trap); element.close(); previous?.focus(); };
  }, []);
  return <dialog ref={dialog} className="dialog" aria-labelledby={titleId}><header className="dialog-heading"><div><h2 id={titleId}>{title}</h2>{description && <p>{description}</p>}</div><button className="icon-button" aria-label="Close dialog" onClick={onClose}><X /></button></header><div className="dialog-body">{children}</div>{footer && <footer className="dialog-footer">{footer}</footer>}</dialog>;
}
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
  const [stopping, setStopping] = useState(false);
  const refresh = useCallback(async () => { const value = await rpc<Snapshot>('snapshot'); setSnapshot(value); }, []);
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
  const quit = () => {
    setStopping(true); setError('');
    void rpc('quit').catch(e => { setStopping(false); setError(message(e)); });
  };
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
  const keepBoth = async (conflict: Conflict) => {
    const wasPaused = paused;
    await action('keep_both', { pairId: conflict.pair_id, path: conflict.path });
    if (!wasPaused) await action('control', { action: 'resume' });
    setNotice('Both versions are preserved. The local copy has a conflict name; the Drive version keeps the original name.');
  };
  const good = !!pair?.enabled && !paused && !pair.status.error && !pair.status.conflicts && !pair.deletion_hold;
  const footer = stopping ? 'Saving progress and stopping…' : paused ? 'Sync is paused' : snapshot?.pairs.some(p => p.enabled) ? 'Sync is running' : 'Choose a folder to start syncing';
  return <div className="app-shell" data-window-visible={snapshot?.window_visible} data-tray-available={snapshot?.tray_available}>
    <aside className="sidebar"><div className="wordmark">FreeSync</div><nav aria-label="Main navigation">{(['Folders', 'Conflicts', 'Preferences'] as Page[]).map(name => {
      const Icon = name === 'Folders' ? Folder : name === 'Conflicts' ? ArrowLeftRight : Settings;
      return <button key={name} className={page === name ? 'nav-link selected' : 'nav-link'} aria-current={page === name ? 'page' : undefined} onClick={() => setPage(name)}><Icon />{name}{name === 'Conflicts' && !!snapshot?.conflicts.length && <span className="count">{snapshot.conflicts.length}</span>}</button>;
    })}</nav><div className="sidebar-bottom"><div className="account-status"><span className="status-dot" /><div><strong>Personal Google account</strong><span>{snapshot?.account_verified ? 'Connected' : snapshot?.account ? 'Checking connection' : 'Not connected'}</span></div></div><div className="sidebar-actions"><button onClick={() => { void action('hide').catch(() => {}); }}><Eye />Hide settings</button><button disabled={busy || stopping} onClick={quit}><Power />Quit</button></div></div></aside>
    <div className="main-shell"><main><header className="page-heading"><div><h1>{page}</h1><p>{page === 'Folders' ? 'Choose what stays in sync.' : page === 'Conflicts' ? 'Preserve both versions when changes overlap.' : 'Make FreeSync work the way you want.'}</p></div><div className="heading-actions"><button disabled={busy || stopping} onClick={() => control(paused ? 'resume' : 'pause')}>{paused ? <Play /> : <Pause />}{paused ? 'Resume' : 'Pause'}</button><button disabled={busy || stopping} onClick={() => control('sync_now')}><RefreshCw />Sync now</button></div></header>
      {error && <div role="alert" className="banner error"><TriangleAlert /><span>{error}</span><button aria-label="Dismiss error" className="icon-button" onClick={() => setError('')}><X /></button></div>}
      {snapshot && !snapshot.window_visible && <div role="status" className="banner"><Eye /><span>Settings are hidden. Sync continues in the tray.</span><button onClick={() => { void action('show').catch(() => {}); }}>Show settings</button></div>}
      {notice && <div role="status" className="banner"><Check /><span>{notice}</span><button aria-label="Dismiss message" className="icon-button" onClick={() => setNotice('')}><X /></button></div>}
      {!snapshot ? <div className="loading"><LoaderCircle className="spin" />Connecting to the background engine…</div> : page === 'Folders' ? <>
        <div className="section-heading"><h2>Your folders</h2><button className="primary" disabled={busy} onClick={() => setPairDialog(true)}><Plus />Add folder</button></div>
        <div className="pair-list">{snapshot.pairs.map(p => <button key={p.id} className={`pair-row ${pair?.id === p.id ? 'selected' : ''}`} onClick={() => setSelected(p.id)} aria-pressed={pair?.id === p.id}><Folder size={27} /><div className="pair-title"><strong>{p.remote_name}</strong><span>Two-way sync</span></div><div className="pair-status"><StatusMark good={p.enabled && !paused && !p.status.conflicts && !p.status.error} />{statusLabel(p, paused)}</div><ChevronRight /></button>)}</div>
        {pair ? <section className="pair-details" aria-label="Selected folder details"><div className="path-columns"><div><label>Local folder</label><p>{compactPath(pair.local_root)}</p></div><div><label>Google Drive folder</label><p>My Drive / {pair.remote_name}</p></div></div><dl className="detail-rows"><div><dt>Last synced</dt><dd>{pair.status.last_sync ? new Date(pair.status.last_sync * 1000).toLocaleString() : 'Not yet'}</dd></div><div><dt>Pending changes</dt><dd>{pair.status.queued || 'None'}</dd></div>{pair.status.current_path && <div><dt>Current work</dt><dd className="work-path">{pair.status.current_path}{pair.status.total_bytes > 0 && <><progress value={pair.status.progress_bytes} max={pair.status.total_bytes} />{bytes(pair.status.progress_bytes)} of {bytes(pair.status.total_bytes)}</>}</dd></div>}{pair.status.retry_at && <div><dt>Next retry</dt><dd>{new Date(pair.status.retry_at * 1000).toLocaleTimeString()}</dd></div>}</dl>
          {pair.status.error && <div role="alert" className="banner error"><TriangleAlert /><span>{pair.status.error.message}</span>{pair.status.state === 'reconnect' && <button onClick={() => setLogin(true)}>Reconnect</button>}</div>}
          {pair.deletion_hold && <div className="banner error"><TriangleAlert /><span>{pair.deletion_count} deletions are paused for review. Review their paths in the preview before continuing.</span><button disabled={busy} onClick={() => { void prepare(pair.id).catch(() => {}); }}>Review</button></div>}
          <div className="detail-actions"><button disabled={busy} onClick={() => { void prepare(pair.id).catch(() => {}); }}>Preview changes{busy && <LoaderCircle className="spin" />}</button><button className="text-button" disabled={busy} onClick={() => setPairDialog(true)}>Folder settings</button></div><p className="muted recovery-note">Deleted files are kept in recovery.</p>
        </section> : <div className="empty"><Folder /><h3>No folders yet</h3><p>Add the test-freesync folder to preview its changes.</p><button className="primary" onClick={() => setPairDialog(true)}>Add folder</button></div>}
      </> : page === 'Conflicts' ? <section className="conflict-list">{snapshot.conflicts.length ? snapshot.conflicts.map(c => <article key={`${c.pair_id}/${c.path}`} className="conflict-row"><ArrowLeftRight /><div><h2>{c.path}</h2><p>{c.reason}</p><p className="muted">Local: {bytes(c.local_bytes)} · Google Drive: {bytes(c.remote_bytes)}</p>{!c.can_keep_both && <p className="muted">This item needs a fresh comparison or a folder rename. Its contents remain preserved.</p>}</div><button disabled={busy || !c.can_keep_both} onClick={() => { void keepBoth(c).catch(() => {}); }}>Keep both</button></article>) : <div className="empty"><Check /><h3>No conflicts</h3><p>Your folder pairs have no unresolved conflicts.</p></div>}</section> : <section className="preferences"><div className="preference-row"><div><h2>Google account</h2><p>{snapshot.account ?? 'Connect an account to browse Drive.'}</p></div><button disabled={busy} onClick={() => setLogin(true)}>{snapshot.account ? 'Reconnect' : 'Connect Google account'}</button></div><div className="preference-row"><div><h2>Start FreeSync at login</h2><p>Keep syncing in the tray when you sign in to this computer.</p></div><input aria-label="Start FreeSync at login" type="checkbox" className="switch" checked={snapshot.autostart} disabled={busy} onChange={e => { void action('preferences', { ...snapshot.preferences, autostart: e.target.checked }).catch(() => {}); }} /></div><div className="preference-row"><div><h2>Notifications</h2><p>Notify you when sync needs attention.</p></div><input aria-label="Notifications" type="checkbox" className="switch" checked={snapshot.preferences.notifications} disabled={busy} onChange={e => { void action('preferences', { notifications: e.target.checked }).catch(() => {}); }} /></div>{snapshot.preferences.notifications && <button onClick={() => { void action('notify_test').then(() => setNotice('A test notification was sent.')).catch(() => {}); }}>Test notification</button>}<div className="preference-row"><div><h2>Recovery files</h2><p className="path-value">{compactPath(snapshot.recovery_directory)}</p><p>Deleted and replaced local files remain here until you remove them yourself.</p></div></div><div className="preference-row"><div><h2>Desktop OAuth client</h2><p>Use a Desktop client from your Google Cloud project.</p></div><button disabled={busy} onClick={() => { void action('import_client').then(value => { if ((value as { imported?: boolean }).imported) setNotice('Desktop client imported. You can connect your account.'); }).catch(() => {}); }}>Import client JSON</button></div></section>}
    </main><footer className="status-footer"><span><StatusMark good={good} />{footer}</span><span>Folder settings are saved automatically</span></footer></div>
    {pairDialog && <PairDialog pair={pair} snapshot={snapshot} onClose={() => setPairDialog(false)} onReconnect={() => setLogin(true)} onSubmit={async (values) => { await action('configure', values); setPairDialog(false); await prepare('test-freesync'); }} />}
    {preview && <Modal title="Preview changes" description="Sync is paused while you review this plan." onClose={closePreview} footer={<><button disabled={busy} onClick={closePreview}>Cancel</button>{pair?.deletion_hold && <button disabled={busy} onClick={() => { void action('approve_deletions', { pairId: preview.pairId, count: pair.deletion_count }).catch(() => {}); }}>Approve {pair.deletion_count} deletions</button>}<button className="primary" disabled={busy || pair?.deletion_hold} onClick={() => { void action('activate', { pairId: preview.pairId }).then(() => { setPreview(undefined); setNotice('Sync is active for the selected folder pair.'); }).catch(() => {}); }}>{snapshot?.pairs.find(p => p.id === preview.pairId)?.enabled ? 'Resume sync' : 'Activate sync'}<ArrowRight /></button></>}><div className="preview-counts"><span><strong>{preview.result.counts.operations}</strong> changes</span><span><strong>{preview.result.counts.conflicts}</strong> conflicts</span><span><strong>{preview.result.counts.skipped}</strong> skipped</span></div><div className="preview-items">{preview.result.operations.map((o, i) => <div key={i}><strong>{o.path}</strong><span>{o.reason}</span></div>)}{preview.result.conflicts.map(c => <div key={c.path}><strong>{c.path}</strong><span>{c.reason}</span></div>)}{preview.result.skipped.map(s => <div key={s.path}><strong>{s.path}</strong><span>{s.reason}</span></div>)}{!preview.result.counts.operations && !preview.result.counts.conflicts && <p>No pending changes. Matching content is already in sync.</p>}</div><p className="muted">Uploads and downloads stay within the selected test folder. Deletions use Drive Trash or local recovery.</p></Modal>}
    {login && <LoginDialog account={snapshot?.account ?? ''} onClose={() => setLogin(false)} onSuccess={() => { setLogin(false); setError(''); setNotice('Google account connected.'); void refresh(); }} />}
  </div>;
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
