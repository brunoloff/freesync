# Implementation progress

Phases 1–8 in `PLAN.md` are complete within the declared Linux preview scope. Phase 8 finished on 9 October 2026 with a real read-only adoption report, tested gradual activation and verified backups. Real-tree activation remains a separate user choice; Phase 9 has started with default-on Git-ignore support; the remaining Phase 9 workflows and later phases are pending.

## Phase 1 — complete

Implemented a Cargo workspace with `freesync-core`, `freesync-google` and `freesync-cli`. Core defines accounts, pairs, fingerprints, provider capabilities, opaque IDs/cursors, operation states, conflicts, typed redacted errors, cancellation, isolated profiles and an exclusive OS file lock shared by desktop/CLI owners. SQLite schema version 1 uses foreign keys, WAL and atomic transactions for inventories, queued work, baselines and consumed cursors.

The fake provider implements paginated listing and change feeds, content transfers, resumable uploads, folders, moves and trash. Faults cover permission denial, timeout, rate limiting, uncertain success and remote changes during download.

Evidence: the full workspace currently passes 46 Rust tests and six Python OAuth-helper tests. Formatting and Clippy with warnings denied pass, including the debug-only desktop bridge. Logs contain only counts, timestamps and typed error codes with bounded rotation. CLI smoke checks passed for help, profile initialization/status, JSON inventory and graceful SIGINT shutdown of a watch process. Locked profile ownership releases on process/object exit. A failed reconciliation transaction leaves the previous inventory/cursor untouched. Profiles remain isolated across reopen.

## Phase 2 — complete

Implemented recursive stable inventory, content fingerprints, exclusions, symlink/special-file handling, unsafe-path/overlap rejection, native recursive watching, bounded debounce, periodic rescans and polling fallback. Errors invalidate the entire inventory instead of inferring deletions.

Evidence: seven local integration tests passed for create/edit/atomic editor save/rename/move/delete/nested folders, hidden files, restart-equivalent scans, exclusions, unreadable root, missing/replaced root, a missed event recovered by polling, root return, native watcher convergence and cancellation. CLI watcher was stopped by SIGINT with exit status 0. Initial policies are documented in `POLICIES.md`.

The native watcher check requires an active native backend and a notification within three seconds while polling is set to thirty seconds. It passed after nested-folder creation and an atomic editor save, proving that polling alone cannot satisfy this check. A separate regression reproduces scan feedback from read/access events and unrelated sibling changes, then verifies that neither requests reconciliation after the fix while real edits still produce native events.

## Phase 3 — complete

Rust OAuth and the read-only Drive adapter now compile. OAuth includes PKCE, state validation, loopback callback, explicit offline consent, refresh, account matching, revocation and native credential stores without plaintext fallback. Linux reads the same Secret Service entry as the completed Python bootstrap. Google adapter supports account identity, metadata/capabilities, all listing/change pages, checksums and bounded range downloads. Initial remote snapshot captures a cursor before traversal and repeats after scoped changes; duplicate names remain visible.

Read-only InSync database inspection established that the existing Crapbox directory corresponds to the account's My Drive root, rather than a Drive folder named Crapbox. Live Drive metadata verified that mapping. The private root IDs are saved outside the source tree. No real-tree write or adoption has been enabled. The disposable local/remote test folder was provisioned and mapped, initially with automatic sync disabled.

Evidence: actual Rust PKCE/loopback consent completed for the expected account. A fresh Rust process read Secret Service, forced token refresh, and verified identity and the change cursor. A root listing followed 16 pages (158 entries before test-folder provisioning). The mapped test-root inventory completed with a change cursor. Four OAuth tests cover state/host injection, duplicate codes, endpoint substitution, missing scopes/refresh tokens, missing/locked credential stores, denied/revoked authorization and transient token failures. Revocation is implemented without deleting synchronized files; live authorization was retained for development.

## Phase 4 — complete

Implemented content-based initial matching, three-way reconciliation, stable-ID/inode moves, parent-first creates, child-first recoverable deletion, explicit duplicate/native-item handling, protected excluded/skipped subtrees, persistent conflicts and safe JSON previews with reasons. The committed change cursor is consumed on later inventories, with expired-cursor scan recovery. Pending work preserves its original expectations instead of being silently replaced by a new scan.

Evidence: six planner tests verify empty repeated matched dry runs, simultaneous edits, ambiguous names, moves with overlapping edits, unknown removal/lost access, replaced roots, blocked folder deletions, exclusions and durable plans across reopen. Four remote-inventory tests cover pagination, unsafe names, listing failure and mutations during traversal. The configured live empty test pair produced an empty durable dry run. Inventories, baselines, conflicts, operations and cursors persist transactionally. Routine previews omit upload session URLs.

## Phase 5 — complete

Implemented guarded Drive folders, resumable creation/update, moves and trash. New IDs are reserved and journaled before creation. The executor stages transfers, verifies checksums and source/destination state, resumes journaled sessions, retains displaced local inodes in private recovery storage, handles uncertain final responses and retries with bounded backoff. Operation execution is deliberately bounded to one concurrent operation, preserving folder dependencies.

Evidence: seven executor tests pass for bidirectional zero-byte/large files, partial upload restart, uncertain completion without duplicates, intervening remote edits, changing downloads, stale local sources, replacements, folder/file moves and persisted trash/recovery. The live transfer suite passed all eight checks: actual stale-ETag rejection, two-way ordinary/zero-byte content, existing-file conditional updates, an interrupted large upload resumed after process restart without duplicates, stable-ID moves, persisted Drive trash and local recovery, preservation of an intervening edit during resumed update, and an unchanged empty queue with the conflict retained. Source-version metadata bumps are accepted for verified downloads while write destinations retain strict conditional checks.

## Phase 6 — complete

The continuous engine compiles and supports native event invalidation, periodic Drive reconciliation, durable pause/resume/sync-now/quit controls, status and retry information, reconnect, offline inventory persistence and a stop for oversized deletion plans. Persistent conflicts allow unrelated paths to continue syncing.

Evidence: four engine tests pass for native changes, pause/resume, restart, exclusive profile ownership, offline recovery, stable queues and unrelated work alongside conflicts. Deletion-review/resume and current-work byte progress are implemented and tested; desktop requests execute between cycles on the sole profile owner. A metadata-only Drive version bump during download has a regression test. The corrected runtime passed all six live automatic-sequence checks: two-way creation, edits/moves, persisted trash/recovery, pause/resume, simultaneous conflicts alongside unrelated transfers, actual network failure through an unreachable local proxy, offline changes, two clean process restarts and verified convergence. The empty queue stayed stable without feedback operations.

On 9 October 2026, the corrected runtime completed a monitored 10,820.76-second native-desktop run. All 17 alternating local/remote checkpoints verified actual disk bytes and downloaded Drive bytes, preserved the intentional conflict and finished with an empty queue. Two scheduled restarts and one UI Quit/restart were clean. The final Quit returned status 0; both the desktop owner and detached supervisor stopped. The private report records success, completion time and graceful shutdown. `scripts/live_soak.py` enforces these checks; elapsed time alone cannot pass it.

Earlier soak attempts are not counted: one ended with its terminal session, and a later supervised run was stopped after a regression exposed scan feedback from read/access events and unrelated sibling changes. The watcher correction ignores those events while retaining real modifications and rescan warnings; the successful sequence and three-hour run used the corrected runtime.

## Phase 7 — complete

Implemented the React/TypeScript settings surface and Tauri 2 backend, folder browsing, controlled test-pair configuration, paused preview/activation, login/reconnect, keep-both conflicts, notification/autostart preferences, hide-on-close and graceful Quit. The interface reads actual Rust state and uses the same serialized owner as the CLI. A debug-only opt-in loopback browser QA bridge calls that native dispatcher; normal builds contain no listener.

Linux UI checks passed real OAuth/reconnect and fresh-process refresh, typed local selection and actual Drive browsing, guarded configuration, preview/activation, pause/resume, verified automatic transfer, keep-both preservation on disk and Drive, native hide/show with continued transfer, and clean UI Quit/restart with saved settings. A competing CLI owner was rejected. Dialog focus wrapping and Escape restoration passed after correcting a focus escape. Nested reconnect names/focus were verified. Actual WebKit DOM rendering and native Tauri IPC were confirmed with an opt-in, process-bound flag/count-only probe excluded from normal builds. Native-reference-size and narrow browser layouts were visually inspected; browser warning/error logs were empty. See [desktop testing](DESKTOP_TESTING.md) for the fidelity ledger and precise native-interaction limitations. Startup/notifications were off during the initial checks; enabling startup then required a separate user choice. Later user selections are retained.

The lifecycle checks were repeated after the watcher correction: UI Quit exited the native process cleanly, and restart preserved the account, active pair, 30-second polling and 25-file deletion limit. Startup and notifications stayed off. The next remote content checkpoint converged while the actual settings window was hidden and the tray object remained present; Show settings restored native visibility.

After the successful soak, the normal frontend and desktop binary were rebuilt without the native probe or browser-test feature. The normal Linux preview started with the saved test pair, three intentionally retained test conflicts and no pending transfers. Phases 1–7 are complete. Broader existing-tree adoption remains Phase 8, and Windows/macOS runtime validation and installers remain Phase 11.

## Desktop refinements — 9 October 2026

Added durable background conflict decisions with individual row progress: Keep both, Use local, Use Drive, Diff and Refresh comparison. Version choices preserve displaced originals in recovery and reject changed sources/destinations. Settings clients can enqueue other choices while the sole owner prepares or transfers a previous choice. Text comparison uses private read-only snapshots, full UTF-8/UTF-16 validation and an installed system viewer; KDiff3 is verified on Linux. Its ordinary unsaved-comparison exit status is accepted, and copies are removed on viewer exit while FreeSync is running; exiting FreeSync first retains the private cache. Added persistent whole-interface zoom, recovery-folder opening, and Linux notification acknowledgement/inhibition feedback.

Evidence: seven new core integration tests cover both version choices and retained bytes, stale local/remote changes, read-only text and binary rejection, durable Keep both recovery, text encodings, and a second database client queuing another decision while the owner is gated. A desktop regression test covers KDiff3's ordinary comparison-close status. The resulting 46 Rust tests pass; formatting, TypeScript/Vite build and full-workspace Clippy with warnings denied pass.

A fresh disposable live subfolder produced five simultaneous-edit conflicts. The UI queued Use local, Use Drive and Keep both while other decisions were working; navigation stayed responsive. Actual local bytes and freshly downloaded Drive bytes verified all three outcomes, their retained recovery originals and the preserved second Keep both name. The previous unrelated conflict stayed unchanged and the transfer queue returned to zero. Diff launched the installed KDiff3 with labeled read-only copies containing the exact two versions; binary content failed visibly without changing originals or leaving its temporary copies. Refresh comparison captured a new comparison without resolving it.

Real Ctrl-wheel input changed zoom to 110%; native WebKit geometry and persisted preferences changed accordingly. UI Quit/restart restored 110%, and Ctrl + 0/reset returned it to 100%. Recovery opening launched the actual file manager at the private recovery directory. The desktop notification service acknowledged the test; an earlier read reported inhibition, while the later test was accepted without reported inhibition. OS popup pixels are not claimed. Existing startup/notification selections were preserved. Desktop and 620px layouts passed visual inspection without horizontal overflow; browser warning/error logs were empty before Quit.

These are focused checks for the refinements. The earlier three-hour run validates the preceding Phase 7 runtime, not a new three-hour run of these refinements. At that checkpoint the preview remained restricted to the authorized test pair. Phase 8 is now complete as recorded below; Phase 10 large-tree continuous scanning and resource validation remain required before recommending full-root use.

## Development environment

Rust 1.96.1, Node 24 and required Linux GTK/WebKit/AppIndicator development libraries are present. Installed the official Clippy component. InSync was not running at the initial process check. The workspace's existing protected `.git` directory is empty and has been preserved; earlier repository publication used an isolated temporary Git checkout.

## Before Phase 8 — Activity and tray refinements

Added durable local activity history and a filterable, sortable Activity tab. History records scans, queued/started/completed transfers, bounded progress checkpoints, conflicts and decisions, recovery, controls, lifecycle and desktop failures. Filters cover literal case-insensitive file name/path, minimum/maximum file size (B/KB/MB/GB), action, outcome and time; eight sort choices and anchored 50-event pages support navigation while new events arrive. Expanded details show exact bytes, operation/decision IDs, attempts, retry/error codes, recovery locations and scan duration/counts. Schema 2 adds history without replacing existing sync state; a private pre-upgrade backup was saved. Detailed rotating JSON logs mirror durable SQLite history; failed mirroring leaves history available and retries later.

Removed Hide settings and Quit from the GUI; Preferences occupies the bottom sidebar, which remains visible alongside long desktop activity lists. Window close still hides settings. Primary tray activation toggles native visibility; the context menu retains Open settings, Pause/Resume, Sync now and Quit. Linux uses a StatusNotifierItem backend because the preceding AppIndicator backend does not deliver primary-click events; other platforms retain the Tauri tray path.

Five focused history tests cover schema upgrade/preserved state, literal/Unicode search, zero and unknown file sizes, safe ranges/sorting, anchored pagination during insertion, restart/deduplication, secret exclusion, log-write failures and rotation. The full Rust suite now has 51 passing tests; frontend builds and warning-free Clippy checks passed. Live Linux and browser checks are recorded in DESKTOP_TESTING.md. At that checkpoint Phase 8 and large-tree validation had not started.

## Phase 8 — complete

The checkpointed read-only adoption manifest verifies the account and root, hashes local files, inventories Drive through a folder census and parent groups, consumes intervening changes and presents matching, proposed-transfer, unresolved, excluded and protected findings. The Adoption tab provides progress, cancellation/resume, literal path search, result filters, pagination, exclusion review and per-folder activation scope.

Activation runs on the sole owner, rejects overlapping roots and changed content or locations, backs up the profile and manifest, installs equivalent baselines and initial conflicts atomically, and creates a root-bound approval receipt. Non-test Google writes require that receipt and verify the reviewed Drive root and ancestry. Existing test-root authorization remains separate. Native link files and known unsafe/unreadable branches stay protected. `ADOPTION.md` describes gradual activation and a stopped-client return to InSync.

### Completed real report

The real read-only Crapbox report finished at 19:38:32 Europe/Lisbon on 9 October 2026, revision 13. All 168,166 Drive folder parents were checked; the manifest contains 900,341 Drive metadata items and 762,637 local entries. The local inventory includes 607,309 files covering 85,347,916,083 bytes. Progress checksum counters are cumulative across resumes and should not be interpreted as unique files.

| Finding | Items | Detail |
| --- | ---: | --- |
| Already synchronized | 759,850 | 604,769 files and 155,081 folders; existing Drive IDs retained on activation |
| Needs review | 2,430 | Ambiguous, differing, unsupported or unreadable items remain unresolved |
| Proposed upload | 578 | 423 files and 155 folders |
| Proposed download | 133,477 | 121,001 files and 12,476 folders |
| Protected | 93 | 7 files and 86 native documents |
| Excluded | 3 | Three branch findings, not three individual files |

The unresolved reasons are 1,981 duplicate-name findings, 203 links/special files, 126 unsupported Drive names, 69 descendants of folder/item-type mismatches, 40 initial content/type differences, 10 unsupported local names and one filesystem permission failure. These findings are preserved for review. The completed report contains 896,431 findings in total and performs no transfers.

The three exclusions cover the managed test mapping, the active FreeSync checkout and one confirmed existing local path outside InSync's saved selection. Account/root mapping was verified against live Drive metadata and the matching InSync account. Saved InSync credentials and guessed patterns were not imported.

### Acceptance evidence

| Requirement | Verified result |
| --- | --- |
| Seeded migration without duplication | A live provider fixture inside the authorized test mapping retained the unchanged Drive ID, installed equivalent baselines without transfers, preserved an intentional mismatch and rejected writes outside its reviewed scope. A later new-file upload was verified by downloading its actual Drive bytes. |
| GUI activation and backups | An isolated GUI fixture installed two equivalent baselines, one mismatch conflict and zero queued transfers. Its receipt bound the selected scope; the profile backup reopened with zero pairs and the manifest backup retained all four findings. Original fixture files were preserved; paired Drive fixture roots went to recoverable Trash. |
| Interrupted inventory and stale comparisons | Core tests and real cancel/resume/restart checks preserved local hashes and remote pages/groups. Changed roots, locations, contents and exclusions reject stale activation. |
| Real report and UI | Name/result filters passed, Needs review pagination advanced from 1–50 to 51–100 of 2,430, and a real folder review showed its exact local/Drive/account scope. Consent stayed unchecked and activation disabled. Return-to-InSync instructions were visible. Browser warning/error logs were empty; no horizontal overflow was found. |
| Activity | The completed Adoption event displayed actual inventory counts, proposed transfers, review count, skipped items, hashed bytes and duration. |
| Completed backups | SQLite backup snapshots of the final profile and manifest passed integrity checks. Counts, ready revision and all verified groups matched the source; the backup includes the completed summary and a private verification record. |
| Normal app restart | The final build without the browser-test feature restarted in the tray. The ready revision and counts, saved test pair and preferences survived. The old QA listener was closed and its stale private launch file removed. InSync remained stopped; no non-test pair was enabled. |

### Corrections validated during the real inventory

Local hashing runs first, retaining useful checkpoints through remote failures. Signed nanosecond timestamps preserve archived dates before the Unix epoch. Local root stamps detect newly added top-level entries both during inventory and before matching.

Drive child listing checks up to four concurrent groups of 64 parents. Single-page groups merge intervening changes; multi-page groups require a quiet cohort window for all changes. An isolated regression using the earlier parent-based rule falsely proposed uploading an unchanged child after a cross-group move; the final rule passed. Checkpoint upgrade retains the census and single-page results while rechecking old multi-page groups. Failed peers and expired child pages preserve completed groups. Consecutive cohorts share their committed cursor, while resume refreshes an idle gap.

Metadata GETs retry transient failures up to five times with bounded exponential backoff and jitter. Short server retry delays are respected; long delays and permanent errors remain visible. Cancellation interrupts the wait without a detached retry. Write protocols retain their existing guards.

Final validation: 77 Rust tests pass, including 19 adoption integration tests and eight grouped-inventory cases; formatting, Clippy with warnings denied, TypeScript and the frontend production build pass. The normal desktop and CLI binaries build successfully. These checks complete Phase 8; they do not establish Phase 10 full-tree continuous-sync performance acceptance. The real tree remains read-only until the user activates reviewed subfolders.


## Phase 9 — Git-ignore policy implemented

Folder options now expose Respect .gitignore, default-on for new and previously saved pairs. Native Rust matchers and a read-only Git index preserve tracked files without invoking Git per file. Both directions, cached baselines, queued transfers, conflict choices and adoption reviews honor the policy. Ignored files remain untouched on both sides, and directory deletion/move guards retain ignored descendants. Watchers retain rule-change coverage; inventories detect changed rule files/indexes and activation requires the reviewed policy signature.

A private read-only audit catalogue records matched remote files and folders, rule provenance, sizes and versions. It separates unknown tracking status and global/info exclusions, reports missing local rule files and unreachable metadata, and performs no cleanup. Current cloud comparison provenance remains the completed Phase 8 metadata snapshot. Reports and real-tree paths/IDs are never published with the source.

Validation for this slice: 84 Rust tests pass, including Git pattern/index coverage, queued-work cancellation, both-direction protection, ignored descendants and adoption review invalidation. Formatting, Clippy with warnings denied, TypeScript and production frontend/native builds pass. Disposable live Drive checks verified included uploads/downloads and left ignored content untouched; GUI checks saved the override into a paused four-operation preview and restoring the option cancelled all four. The completed real-tree report was reclassified from the saved metadata inventory without Drive writes. Windows/macOS runtime checks and Phase 10 continuous-sync performance remain pending.
