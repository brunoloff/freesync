# Implementation progress

Goal: complete all Phase 1–7 requirements in `PLAN.md`. A phase is complete only after its acceptance checks have current evidence.

## Phase 1 — complete

Implemented a Cargo workspace with `freesync-core`, `freesync-google` and `freesync-cli`. Core defines accounts, pairs, fingerprints, provider capabilities, opaque IDs/cursors, operation states, conflicts, typed redacted errors, cancellation, isolated profiles and an exclusive OS file lock shared by desktop/CLI owners. SQLite schema version 1 uses foreign keys, WAL and atomic transactions for inventories, queued work, baselines and consumed cursors.

The fake provider implements paginated listing and change feeds, content transfers, resumable uploads, folders, moves and trash. Faults cover permission denial, timeout, rate limiting, uncertain success and remote changes during download.

Evidence: the full workspace currently passes 37 Rust tests and six Python OAuth-helper tests. Formatting and Clippy with warnings denied pass, including the debug-only desktop bridge. Logs contain only counts, timestamps and typed error codes with bounded rotation. CLI smoke checks passed for help, profile initialization/status, JSON inventory and graceful SIGINT shutdown of a watch process. Locked profile ownership releases on process/object exit. A failed reconciliation transaction leaves the previous inventory/cursor untouched. Profiles remain isolated across reopen.

## Phase 2 — complete

Implemented recursive stable inventory, content fingerprints, exclusions, symlink/special-file handling, unsafe-path/overlap rejection, native recursive watching, bounded debounce, periodic rescans and polling fallback. Errors invalidate the entire inventory instead of inferring deletions.

Evidence: six local integration tests passed for create/edit/atomic editor save/rename/move/delete/nested folders, hidden files, restart-equivalent scans, exclusions, unreadable root, missing/replaced root, a missed event recovered by polling, root return, native watcher convergence and cancellation. CLI watcher was stopped by SIGINT with exit status 0. Initial policies are documented in `POLICIES.md`.

## Phase 3 — complete

Rust OAuth and the read-only Drive adapter now compile. OAuth includes PKCE, state validation, loopback callback, explicit offline consent, refresh, account matching, revocation and native credential stores without plaintext fallback. Linux reads the same Secret Service entry as the completed Python bootstrap. Google adapter supports account identity, metadata/capabilities, all listing/change pages, checksums and bounded range downloads. Initial remote snapshot captures a cursor before traversal and repeats after scoped changes; duplicate names remain visible.

Read-only InSync and Drive inspection established the existing-tree mapping. The private root IDs are saved outside the source tree. No real-tree write or adoption has been enabled. The disposable local/remote test folder was provisioned and mapped, initially with automatic sync disabled.

Evidence: actual Rust PKCE/loopback consent completed for the expected account. A fresh Rust process read Secret Service, forced token refresh, and verified identity and the change cursor. A root listing followed 16 pages (158 entries before test-folder provisioning). The mapped test-root inventory completed with a change cursor. Four OAuth tests cover state/host injection, duplicate codes, endpoint substitution, missing scopes/refresh tokens, missing/locked credential stores, denied/revoked authorization and transient token failures. Revocation is implemented without deleting synchronized files; live authorization was retained for development.

## Phase 4 — complete

Implemented content-based initial matching, three-way reconciliation, stable-ID/inode moves, parent-first creates, child-first recoverable deletion, explicit duplicate/native-item handling, protected excluded/skipped subtrees, persistent conflicts and safe JSON previews with reasons. The committed change cursor is consumed on later inventories, with expired-cursor scan recovery. Pending work preserves its original expectations instead of being silently replaced by a new scan.

Evidence: six planner tests verify empty repeated matched dry runs, simultaneous edits, ambiguous names, moves with overlapping edits, unknown removal/lost access, replaced roots, blocked folder deletions, exclusions and durable plans across reopen. Four remote-inventory tests cover pagination, unsafe names, listing failure and mutations during traversal. The configured live empty test pair produced an empty durable dry run. Inventories, baselines, conflicts, operations and cursors persist transactionally. Routine previews omit upload session URLs.

## Phase 5 — complete

Implemented guarded Drive folders, resumable creation/update, moves and trash. New IDs are reserved and journaled before creation. The executor stages transfers, verifies checksums and source/destination state, resumes journaled sessions, retains displaced local inodes in private recovery storage, handles uncertain final responses and retries with bounded backoff. Operation execution is deliberately bounded to one concurrent operation, preserving folder dependencies.

Evidence: seven executor tests pass for bidirectional zero-byte/large files, partial upload restart, uncertain completion without duplicates, intervening remote edits, changing downloads, stale local sources, replacements, folder/file moves and persisted trash/recovery. The live transfer suite passed all eight checks: actual stale-ETag rejection, two-way ordinary/zero-byte content, existing-file conditional updates, an interrupted large upload resumed after process restart without duplicates, stable-ID moves, persisted Drive trash and local recovery, preservation of an intervening edit during resumed update, and an unchanged empty queue with the conflict retained. Source-version metadata bumps are accepted for verified downloads while write destinations retain strict conditional checks.

## Phase 6 — in progress

The continuous engine compiles and supports native event invalidation, periodic Drive reconciliation, durable pause/resume/sync-now/quit controls, status and retry information, reconnect, offline inventory persistence and a stop for oversized deletion plans. Persistent conflicts allow unrelated paths to continue syncing.

Evidence: four engine tests pass for native changes, pause/resume, restart, exclusive profile ownership, offline recovery, stable queues and unrelated work alongside conflicts. Deletion-review/resume and current-work byte progress are implemented and tested; desktop requests execute between cycles on the sole profile owner. The live automatic sequence caught a metadata-only Drive version bump during download; the fix has a regression test. The fresh live sequence has passed two-way creation, edits/moves, persisted trash/recovery, pause/resume, and simultaneous conflicts alongside unrelated transfers. Actual network-failure/restart and stable-queue checks are running; the monitored three-hour soak has not started yet.

## Phase 7 — in progress

Implemented the React/TypeScript settings surface and Tauri 2 backend, folder browsing, controlled test-pair configuration, paused preview/activation, login/reconnect, keep-both conflicts, notification/autostart preferences, hide-on-close and graceful Quit. The interface reads actual Rust state and uses the same serialized owner as the CLI. A debug-only opt-in loopback browser QA bridge calls that native dispatcher; normal builds contain no listener. Linux rendered/engine UI acceptance and tray checks remain outstanding. The goal remains active.

## Development environment

Rust 1.96.1, Node 24 and required Linux GTK/WebKit/AppIndicator development libraries are present. Installed the official Clippy component. InSync was not running at the initial process check. The workspace's existing protected `.git` directory is empty and has been preserved; earlier repository publication used an isolated temporary Git checkout.
