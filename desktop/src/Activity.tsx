import { Fragment, useEffect, useState } from 'react';
import { ChevronDown, ChevronRight, Folder, LoaderCircle, RefreshCw, Search } from 'lucide-react';
import { message, rpc, type ActivityEntry, type ActivityPage } from './api';

const actions = ['upload', 'download', 'create_drive_folder', 'create_local_folder', 'move_drive', 'move_local', 'drive_trash', 'local_recovery', 'recovery', 'conflict', 'keep_both', 'use_local', 'use_drive', 'compare', 'refresh_comparison', 'scan', 'sync', 'adoption', 'control', 'engine', 'desktop'];
const outcomes = ['queued', 'started', 'progress', 'completed', 'cancelled', 'retry', 'conflict', 'error', 'info'];
const label = (value: string) => value.replaceAll('_', ' ').replace(/^./, c => c.toUpperCase());
function size(value?: number | null): string {
  if (value == null) return '—';
  if (value < 1024) return `${value} B`;
  for (const [unit, scale] of [['TB', 1024 ** 4], ['GB', 1024 ** 3], ['MB', 1024 ** 2], ['KB', 1024]] as const) {
    if (value >= scale) return `${(value / scale).toFixed(1)} ${unit}`;
  }
  return `${value} B`;
}
interface Filters { name: string; minimum: string; maximum: string; unit: number; action: string; outcome: string; period: string; sort: string }
const initial: Filters = { name: '', minimum: '', maximum: '', unit: 1024 * 1024, action: '', outcome: '', period: '', sort: 'newest' };
export default function Activity() {
  const [filters, setFilters] = useState(initial);
  const [page, setPage] = useState(0);
  const [anchor, setAnchor] = useState<number>();
  const [data, setData] = useState<ActivityPage>();
  const [expanded, setExpanded] = useState<number>();
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [loading, setLoading] = useState(true);
  const [refresh, setRefresh] = useState(0);
  const update = (change: Partial<Filters>) => { setData(undefined); setLoading(true); setFilters(previous => ({ ...previous, ...change })); setPage(0); setAnchor(undefined); setExpanded(undefined); };
  useEffect(() => {
    let cancelled = false; let timer: ReturnType<typeof setTimeout>;
    const minimum = filters.minimum === '' ? undefined : Math.round(Number(filters.minimum) * filters.unit);
    const maximum = filters.maximum === '' ? undefined : Math.round(Number(filters.maximum) * filters.unit);
    const invalid = [minimum, maximum].some(v => v !== undefined && (!Number.isSafeInteger(v) || v < 0)) || minimum !== undefined && maximum !== undefined && minimum > maximum;
    setLoading(true); setError('');
    if (invalid) { setError('Enter a valid size range: minimum must not exceed maximum.'); setLoading(false); return; }
    async function load() {
      try {
        const value = await rpc<ActivityPage>('activity', { name: filters.name, min_bytes: minimum, max_bytes: maximum, action: filters.action || undefined, outcome: filters.outcome || undefined, since_ms: filters.period ? Date.now() - Number(filters.period) * 86400000 : undefined, sort: filters.sort, offset: page * 50, limit: 50, until_id: anchor });
        if (!cancelled) { setData(value); setError(''); }
      } catch (e) { if (!cancelled) setError(message(e)); }
      finally { if (!cancelled) { setLoading(false); if (page === 0) timer = setTimeout(load, 5000); } }
    }
    timer = setTimeout(() => { void load(); }, 200);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [filters, page, anchor, refresh]);
  const openLogs = () => { void rpc('open_logs').then(() => setNotice('Application logs folder opened.')).catch(e => setError(message(e))); };
  return <section className="activity" aria-label="Activity history">
    <div className="section-heading"><div><h2>Past activity</h2><p className="muted">History is stored locally and starts with activity logging. The newest page updates automatically.</p></div><div className="activity-tools"><button onClick={() => { setPage(0); setAnchor(undefined); setRefresh(v => v + 1); }}><RefreshCw />Refresh</button><button onClick={openLogs}><Folder />Open logs folder</button></div></div>
    {notice && <p role="status" className="banner">{notice}</p>}
    <div className="activity-filters" aria-label="Filter activity">
      <div className="activity-search"><label htmlFor="activity-name">File name or path</label><div className="search-field"><Search /><input id="activity-name" type="search" placeholder="Search activity…" value={filters.name} onChange={e => update({ name: e.target.value })} /></div></div>
      <div className="activity-size"><label>File size</label><div><input aria-label="Minimum file size" type="number" min="0" step="any" placeholder="Minimum" value={filters.minimum} onChange={e => update({ minimum: e.target.value })} /><span>to</span><input aria-label="Maximum file size" type="number" min="0" step="any" placeholder="Maximum" value={filters.maximum} onChange={e => update({ maximum: e.target.value })} /><select aria-label="File size unit" value={filters.unit} onChange={e => update({ unit: Number(e.target.value) })}><option value={1}>B</option><option value={1024}>KB</option><option value={1024 ** 2}>MB</option><option value={1024 ** 3}>GB</option></select></div></div>
      <div><label htmlFor="activity-action">Action</label><select id="activity-action" value={filters.action} onChange={e => update({ action: e.target.value })}><option value="">All actions</option>{actions.map(a => <option key={a} value={a}>{label(a)}</option>)}</select></div>
      <div><label htmlFor="activity-outcome">Outcome</label><select id="activity-outcome" value={filters.outcome} onChange={e => update({ outcome: e.target.value })}><option value="">All outcomes</option>{outcomes.map(a => <option key={a} value={a}>{label(a)}</option>)}</select></div>
      <div><label htmlFor="activity-period">Period</label><select id="activity-period" value={filters.period} onChange={e => update({ period: e.target.value })}><option value="">All time</option><option value="1">Last 24 hours</option><option value="7">Last 7 days</option><option value="30">Last 30 days</option></select></div>
      <div><label htmlFor="activity-sort">Sort by</label><select id="activity-sort" value={filters.sort} onChange={e => update({ sort: e.target.value })}>{[['newest', 'Newest first'], ['oldest', 'Oldest first'], ['name_asc', 'Name A–Z'], ['name_desc', 'Name Z–A'], ['size_desc', 'Largest first'], ['size_asc', 'Smallest first'], ['action_asc', 'Action'], ['outcome_asc', 'Outcome']].map(([value, text]) => <option value={value} key={value}>{text}</option>)}</select></div>
      <button className="text-button" onClick={() => update(initial)}>Clear filters</button>
    </div>
    {error && <p role="alert" className="banner error">{error}</p>}
    {data?.log_error && <p role="alert" className="banner error">Activity history is saved, but the application log file could not be updated: {data.log_error.message}</p>}
    <div className="activity-summary" role="status"><span>{data ? `${data.matching.toLocaleString()} matching events · ${data.total.toLocaleString()} recorded` : 'Loading activity…'}</span>{loading && <span><LoaderCircle className="spin" />Updating…</span>}</div>
    {data?.entries.length ? <table className="activity-table"><thead><tr><th>Time</th><th>Action / file</th><th>Size</th><th>Outcome</th></tr></thead><tbody>{data.entries.map(entry => <Fragment key={entry.id}>
      <tr><td><time dateTime={new Date(entry.at_ms).toISOString()} title={new Date(entry.at_ms).toISOString()}>{new Date(entry.at_ms).toLocaleString()}</time></td><td><button className="activity-event" aria-expanded={expanded === entry.id} aria-controls={`activity-details-${entry.id}`} aria-label={`Details for event ${entry.id}: ${entry.path || label(entry.action)}`} onClick={() => setExpanded(previous => previous === entry.id ? undefined : entry.id)}>{expanded === entry.id ? <ChevronDown /> : <ChevronRight />}<span><strong>{label(entry.action)}</strong><span>{entry.path || entry.message}</span></span></button></td><td title={entry.size_bytes == null ? 'No file size applies' : `${entry.size_bytes} bytes`}>{size(entry.size_bytes)}</td><td><span className="activity-outcome" data-outcome={entry.outcome}>{label(entry.outcome)}</span></td></tr>
      {expanded === entry.id && <tr id={`activity-details-${entry.id}`} className="activity-detail"><td colSpan={4}><EntryDetails entry={entry} /></td></tr>}
    </Fragment>)}</tbody></table> : !loading && <div className="empty"><Search /><h3>{data?.total ? 'No matching activity' : 'No activity recorded yet'}</h3><p>{data?.total ? 'Try a different name, size range or outcome.' : 'New sync work and application events will appear here.'}</p></div>}
    <div className="activity-pagination"><span>{data?.matching ? `Showing ${page * 50 + 1}–${Math.min(page * 50 + data.entries.length, data.matching)}` : 'Showing 0 events'}</span><div><button disabled={loading || page === 0} onClick={() => { if (page === 1) setAnchor(undefined); setPage(p => p - 1); setExpanded(undefined); }}>Previous</button><button disabled={loading || !data || (page + 1) * 50 >= data.matching} onClick={() => { setAnchor(data?.latest_id); setPage(p => p + 1); setExpanded(undefined); }}>Next</button></div></div>
  </section>;
}
function EntryDetails({ entry }: { entry: ActivityEntry }) {
  const d = entry.details;
  const values: [string, string | number | undefined | null][] = [
    ['Event', entry.id], ['Time', new Date(entry.at_ms).toISOString()], ['Folder pair', entry.pair_id], ['File', entry.path], ['Description', entry.message],
    ['Operation / decision ID', d.operation_id], ['Previous path', d.from_path], ['Preserved copy', d.recovery_path], ['Attempt', d.attempt],
    ['Local version', d.local_bytes == null ? undefined : `${size(d.local_bytes)} (${d.local_bytes} bytes)`], ['Drive version', d.drive_bytes == null ? undefined : `${size(d.drive_bytes)} (${d.drive_bytes} bytes)`],
    [entry.action === 'adoption' ? 'Hashed' : 'Transferred', d.bytes_done == null ? undefined : `${size(d.bytes_done)} (${d.bytes_done} bytes)`], ['Retry at', d.retry_at ? new Date(d.retry_at * 1000).toLocaleString() : undefined],
    ['Error code', d.error_code], ['Duration', d.duration_ms == null ? undefined : `${d.duration_ms.toLocaleString()} ms`], [entry.action === 'adoption' ? 'Proposed transfers' : 'Pending changes', d.changes], [entry.action === 'adoption' ? 'Needs review' : 'Conflicts', d.conflicts], ['Skipped items', d.skipped],
  ];
  return <dl>{values.filter(([, value]) => value != null).map(([name, value]) => <div key={name}><dt>{name}</dt><dd>{value}</dd></div>)}</dl>;
}
