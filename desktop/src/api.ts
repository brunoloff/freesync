import { invoke } from '@tauri-apps/api/core';
export const native = '__TAURI_INTERNALS__' in window;
export interface AppError { code: string; message: string }
export async function rpc<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (native) return invoke<T>('settings', { command, args });
  const response = await fetch('/api/command', { method: 'POST', headers: { 'Content-Type': 'application/json', 'X-FreeSync': '1' }, body: JSON.stringify({ command, args }) });
  const value = await response.json();
  if (!response.ok) throw value.error ?? { code: 'io', message: 'FreeSync stopped or the settings connection expired. Open settings again.' };
  return value.ok as T;
}
export function message(error: unknown): string {
  return error && typeof error === 'object' && 'message' in error && typeof error.message === 'string' ? error.message : 'The settings connection is unavailable. Reopen FreeSync and try again.';
}
export interface Status { state: string; queued: number; conflicts: number; last_sync?: number; error?: AppError; current_path?: string; progress_bytes: number; total_bytes: number; retry_at?: number }
export interface Pair { id: string; account: string; local_root: string; remote_id: string; remote_name: string; remote_path?: string; enabled: boolean; poll_secs: number; deletion_limit: number; status: Status; deletion_hold: boolean; deletion_count: number }
export type ConflictChoice = 'keep_both' | 'use_local' | 'use_drive' | 'compare' | 'refresh';
export interface Conflict { id: string; pair_id: string; path: string; reason: string; can_keep_both: boolean; can_use_local: boolean; can_use_drive: boolean; local_bytes?: number; remote_bytes?: number }
export interface ConflictJob { id: string; conflict_id: string; pair_id: string; path: string; choice: ConflictChoice; state: string; error?: AppError }
export interface Snapshot { account?: string; account_verified: boolean; pairs: Pair[]; conflicts: Conflict[]; conflict_jobs: ConflictJob[]; controls: { paused: boolean; quit: boolean }; preferences: { notifications: boolean; zoom: number }; autostart: boolean; recovery_directory: string; window_visible: boolean; tray_available: boolean; engine_error?: AppError; native_ui?: { mounted: boolean; ipc_verified: boolean } }
export interface Preview { counts: { operations: number; conflicts: number; skipped: number; matched: number }; operations: { path: string; action: { kind: string }; reason: string }[]; conflicts: { path: string; reason: string }[]; skipped: { path: string; reason: string }[]; matched: { path: string; reason: string }[] }
export interface Folder { id: string; name: string; writable: boolean }
export interface FolderPage { id: string; name: string; folders: Folder[]; next?: string }
export interface ActivityEntry { id: number; at_ms: number; action: string; outcome: string; pair_id?: string; path?: string; size_bytes?: number; message: string; details: { operation_id?: string; from_path?: string; recovery_path?: string; attempt?: number; local_bytes?: number; drive_bytes?: number; bytes_done?: number; retry_at?: number; error_code?: string; duration_ms?: number; changes?: number; conflicts?: number; skipped?: number } }
export interface ActivityPage { entries: ActivityEntry[]; matching: number; total: number; latest_id: number; limit: number; log_error?: AppError }
export interface AdoptionFinding { path: string; status: string; reason: string; size_bytes?: number; remote_id?: string; kind?: 'file' | 'folder' | 'native_document' | 'shortcut' }
export interface AdoptionReport { source: { account_email: string; local_root: string; remote_root_id: string; root_identity: string; excludes: string[]; exclusion_source: string }; progress: { phase: string; revision: number; drive_passes?: number; remote_folders_checked?: number; remote_folders_total?: number; remote_items: number; local_items: number; hashed_bytes: number; reused_hashes: number; current_path?: string; completed_at?: number; error?: AppError }; counts: Record<string, number>; findings: AdoptionFinding[]; matching: number; offset: number; directory: string }
export interface AdoptionScope { scope: string; revision: number; pair: { local_root: string; remote_root_id: string; remote_root_name: string; account_email: string; excludes: string[] }; counts: Record<string, number> }
