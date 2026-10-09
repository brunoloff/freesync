# FreeSync implementation roadmap

FreeSync will synchronize selected local folders with Google Drive on Linux, Windows, and macOS. The implementation will use a Rust sync engine, SQLite for persistent state, and a Tauri 2 desktop interface. The engine will run independently of the settings window and expose the same operations through a command line interface for testing and recovery.

This roadmap defines a complete first version and stable stopping points for implementation goals. Phases are cumulative: completing Phase 7 means completing Phases 1 through 7 and their acceptance checks. Current evidence and remaining acceptance checks are tracked in [implementation progress](PROGRESS.md). OAuth setup is documented separately in [OAuth setup](OAUTH_SETUP.md).

## Scope and existing files

The first version includes automatic two-way sync, multiple Google accounts and folder pairs, selective sync, ignore rules, conflict handling, recoverable deletion, offline recovery, a tray interface, startup at login, and installers for the three desktop platforms. Google-native documents will have an explicit link-file policy; ordinary files will retain their bytes without automatic format conversion.

OneDrive and other providers come after this roadmap. The architecture will allow them, but implementing another provider is not required for version 1. Online-only placeholders, filesystem mounts, file-manager overlays, shared-drive administration, and editing Google-native documents through local office files are also later extensions.

The user already has an InSync-managed tree at `the existing sync tree`. He intends to turn off InSync before live FreeSync testing. FreeSync must verify that it is not competing with another sync process before starting a live test or adoption. The corresponding Google account has been confirmed through Drive OAuth and is recorded in private local configuration. The remote mapping was established through read-only InSync and Drive inspection, as recorded in `PROGRESS.md`; adoption must still carry over and review existing InSync exclusions. Do not infer a mapping from a folder name alone.

Existing files under `the existing sync tree` may be used for read-only inventory and comparison. Development may create, edit, rename, and remove test files under `the configured disposable test-freesync folder` and its explicitly paired remote test folder. Prefer a separate directory for each test run and clean up only that run's fixtures. Live integration tests must check both the local files and persisted Drive state.

Until the user enables broader sync, automatic test mutations stay within those test roots. Implementation of adoption can finish with a real read-only inventory and successful migration tests in the sandbox; activating writes across the real existing sync tree tree is a separate user action or explicit instruction.

## Architecture and rules that apply throughout

Use a Cargo workspace with separate modules or crates for the provider-independent engine, the Google Drive adapter, and the CLI. Add the desktop application in Phase 7. The proposed supporting libraries are Tokio, notify, reqwest, Serde, and rusqlite; validate compatible versions when scaffolding.

The provider interface should expose remote identity, listing, change cursors, content transfer, folder creation, moves, and trash operations. Keep capabilities explicit, including checksums, permissions, native documents, and resumable transfers. Do not force future providers to pretend that their IDs, cursors, or semantics match Google's.

The engine compares current local and remote state with the last successfully synchronized state. SQLite stores account identity, remote file IDs, path mappings, fingerprints, change cursors, conflicts, and pending operations. Store application state outside synchronized folders, and keep credentials in the OS credential store.

These rules apply before any live writes:

- Filesystem notifications request reconciliation; they are not a complete record of what happened. Combine notifications with scans and recovery after overflow or missed events.
- Treat a missing item as a deletion only with evidence from an established baseline. An unavailable disk, changed root, lost permission, or failed listing must pause the affected pair rather than propagate deletion.
- Preserve both sides of simultaneous edits and ambiguous initial matches. Do not resolve conflicts using timestamps alone.
- Confine operations to validated local roots and remote folder IDs. Initially skip symbolic links; reject path traversal, directory escapes, and overlapping sync roots.
- Keep a durable operation queue. Reconcile uncertain API outcomes before retrying operations that might create duplicates.
- Download into staging files, verify the result, then replace the intended file. Recheck whether either side changed during transfer.
- Use Drive trash and local recovery storage for propagated deletions. Version 1 will not permanently delete remote files as part of normal sync.
- Persist consumed remote changes and resulting work before advancing the stored cursor. Recovery must not forget operations after a crash.
- Apply exclusions to both scans and watchers. Removing a folder from selective sync must have an explicit keep-local-files default and must not trigger remote deletion.
- Keep tokens, file contents, and private paths out of routine diagnostic output. A support export must show a preview of the information it includes.

## Phase overview

| Phase | Deliverable | Useful stopping point |
| --- | --- | --- |
| 1 | Engine foundation and test harness | An offline development base |
| 2 | Local inventory and folder monitoring | Reliable local change detection |
| 3 | Google authentication and read-only adapter | Browse the real account safely |
| 4 | Persistent reconciliation and dry runs | Preview what would sync |
| 5 | Controlled transfers and recovery | Manually execute a tested sync plan |
| 6 | Automatic sync in the test folder | A working sync engine through the CLI |
| 7 | Desktop interface and tray | A usable desktop preview |
| 8 | Adoption of the existing InSync tree | Ready to use existing existing sync tree files |
| 9 | Complete Google Drive workflows | Feature-complete version 1 |
| 10 | Recovery, scale, and performance validation | A release candidate engine |
| 11 | Cross-platform validation and installers | Installable desktop previews |
| 12 | Release readiness and handover | A complete version 1 app |

## Phase 1 Engine foundation and test harness

Create the development structure and define the data model before touching the real account.

**Deliverables**

- Inspect current tooling, establish the Rust workspace, and preserve any existing repository metadata. Add configuration, logging, typed errors, cancellation, and a lock preventing two engine instances from owning the same profile.
- Define accounts, folder pairs, remote identities, fingerprints, operation states, conflicts, and provider capabilities. Introduce versioned SQLite migrations and transactions.
- Build a fake cloud provider and temporary local fixtures. It must simulate permission failures, timeouts, rate limits, uncertain outcomes, and changes during operations.
- Add a CLI with help, profile inspection, and machine-readable output. Document the initial policies for hidden files, links, exclusions, and name collisions.

**Complete when** the workspace builds, formatting and static checks pass, database migrations work, and meaningful tests demonstrate profile isolation and the fake provider. No live Drive mutation is needed.

## Phase 2 Local inventory and folder monitoring

Build a reliable view of a selected local tree.

**Deliverables**

- Recursively inventory ordinary files and directories, recording normalized relative paths, size, timestamps, and content fingerprints when needed. Skip links and report unsupported entries.
- Monitor existing and newly created subdirectories with notify. Debounce bursts, recognize editor saves that replace files, and defer files whose contents are still changing.
- Rescan at startup and after watcher failure, overflow, or root reappearance. Provide a polling fallback and consistent exclusion handling.
- Expose local changes and scanner errors through the CLI without uploading anything.

**Complete when** create, edit, atomic save, rename, move, delete, nested-folder creation, and restart scenarios converge to the actual disk state. Tests must include a missed notification, an inaccessible root, and a root that temporarily disappears. Watchers have documented limitations that motivate scan recovery. [notify documentation](https://docs.rs/notify/latest/notify/)

## Phase 3 Google authentication and read-only adapter

Connect FreeSync to the intended Google account and identify the actual remote roots.

**Deliverables**

- Set up a Google Cloud project with the Drive API, consent configuration, and a desktop OAuth client. Record the manual setup steps and keep credentials out of version control.
- Implement login through the system browser using PKCE, state validation, and a loopback callback; support refresh, expiry, revocation, logout, and unavailable credential stores. [Google desktop OAuth](https://developers.google.com/identity/protocols/oauth2/native-app)
- Verify scopes against the requirement to synchronize existing files. Google classifies broad `drive` access as restricted, while `drive.file` grants per-file app access; record applicable publishing requirements without assuming an app-created folder provides access to an existing tree. [Drive scopes](https://developers.google.com/workspace/drive/api/guides/api-specific-auth)
- Implement account identity, paginated folder listing, metadata, permission checks, and incremental change reads. Select roots by file ID and preserve their ancestry.
- Identify the remote counterpart of `test-freesync` and record a separate read-only mapping for the existing existing sync tree tree. Inventory any accessible InSync configuration without editing it.

**Complete when** real login succeeds, the expected account identity is confirmed, and metadata for the intended folders is retrieved across pagination. Token refresh and failure handling must be exercised. Fake-provider success alone does not complete this phase.

**External prerequisite:** Bruno may need to create or choose the Google Cloud project and complete consent in his browser. The app needs its own OAuth authorization; an unrelated connector login is not a substitute.

## Phase 4 Persistent reconciliation and dry runs

Explain exactly what a sync would do without executing it.

**Deliverables**

- Persist local and remote inventories, file ID mappings, successful baselines, conflicts, and queued operations.
- Implement a planner producing uploads, downloads, creates, moves, recoverable deletions, conflicts, skips, and no-ops with reasons.
- Define initial matching separately from ongoing sync. Equal timestamps or filenames alone are insufficient evidence of equal content; obtain comparable content evidence where needed and keep unresolved matches explicit.
- Capture the remote change token before initial traversal, consume changes that occur during the traversal, and repeat reconciliation until the inventory is consistent. Drive supports incremental reads using stored page tokens. [Drive change tracking](https://developers.google.com/workspace/drive/api/guides/manage-changes)
- Add CLI previews with counts and per-item explanations. Cache enough ancestry to distinguish changes within a selected root from unrelated account changes.

**Complete when** repeated dry runs on unchanged fixtures are empty, simultaneous edits become conflicts, and restart preserves the plan. Missing roots, revoked access, duplicate names, and an incomplete scan must never produce inferred mass deletion. A change-feed removal can mean lost access rather than deletion. [Drive change resource](https://developers.google.com/workspace/drive/api/reference/rest/v3/changes)

## Phase 5 Controlled transfers and recovery

Execute an explicit plan in the test folder before enabling continuous sync.

**Deliverables**

- Implement folder creation, uploads, downloads, metadata updates, moves, and trash operations, guarded by the selected test roots.
- Add bounded concurrency, timeouts, retries with backoff, resumable uploads, and staged downloads with integrity verification. [Drive uploads](https://developers.google.com/workspace/drive/api/guides/manage-uploads)
- Revalidate source fingerprints and destination state before replacing contents. Preserve conflicts if a file changes during transfer.
- Journal operation progress, recognize uncertain remote success, and retain deleted or replaced local content according to a documented recovery policy.

**Complete when** live fixtures transfer in both directions with verified content, including zero-byte files and a file large enough to exercise interrupted transfer recovery. Restart after interruption must converge without duplicate Drive objects or silently overwriting intervening edits. Test rename and trash outcomes by reading persisted remote state.

## Phase 6 Automatic sync in the test folder

Combine monitoring, change polling, planning, and execution into a continuous engine.

**Deliverables**

- Run local reconciliation and configurable Drive polling automatically, with pause, resume, sync-now, and graceful shutdown commands.
- Persist unfinished work while offline. Recover after network loss, restart, authentication failure, or watcher failure.
- Recognize our own local and remote writes to prevent feedback loops. Keep pending conflicts visible while unrelated files continue syncing.
- Add per-pair status, queue progress, retry information, and an automatic stop for unexpected large deletion plans. Require a healthy scan and revalidated baseline before resuming.

**Complete when** a live test sequence under `test-freesync` covers creation, editing, moves, deletion, simultaneous local and remote edits, offline changes, and process restart. Verify final disk and Drive content, preserved conflicts, and an empty stable queue. Follow with a monitored soak run spanning several hours and multiple restarts; elapsed time alone is insufficient evidence.

**Milestone:** a working automatic sync engine, operated through the CLI. This is the first substantial implementation goal to target.

## Phase 7 Desktop interface and tray

Make the engine usable as an everyday desktop application.

**Deliverables**

- Add Tauri 2 with TypeScript and a modest settings UI. Keep filesystem operations and credentials in the Rust side behind narrowly scoped commands.
- Provide login, account identification, remote folder browsing, local folder selection, dry-run preview, and controlled activation of a folder pair.
- Show tray status, current work, errors, conflicts, and pause/resume controls. Closing the settings window keeps the engine running; choosing Quit stops it cleanly.
- Offer startup at login, notifications, reconnect, and a basic conflict workflow preserving both versions. Handle keyboard access and readable error messages.
- Keep one profile owner across CLI and desktop entrypoints. For version 1, a tray-resident user process is sufficient; a privileged service is unnecessary.

**Complete when** login, configuration, preview, activation, automatic sync, pause/resume, conflict handling, window close, and Quit work through the UI on Linux. Persisted settings must survive restart. Test the rendered UI and actual engine behavior, not just screenshots. Tauri supports desktop packaging and tray interfaces. [Tauri architecture](https://v2.tauri.app/concept/architecture/)

**Milestone:** a usable desktop preview, initially exercised within the test folder.

## Phase 8 Adoption of the existing InSync tree

Reuse already synchronized files without treating the tree as a fresh upload or download.

**Deliverables**

- Add an adoption workflow that verifies account and root identity, inventories both sides, carries over confirmed exclusions, and presents initial discrepancies.
- Match existing local content to remote file IDs. Record equivalent files as already synchronized; preserve ambiguous duplicates and changed files as unresolved differences.
- Establish a consistent baseline despite changes occurring during scanning. Resuming after cancellation must preserve verified matches and recheck stale comparisons.
- Start adoption with a read-only report for the real `the existing sync tree` tree. Separate matching from execution and support gradually enabling selected subfolders.
- Show how to pause or exit FreeSync and return to InSync without erasing either side. Keep a backup of FreeSync configuration and its adoption manifest.

**Complete when** a seeded migration fixture adopts unchanged files without transfer or duplicate creation, isolates intentional mismatches, and survives interruption. Produce a real read-only existing sync tree adoption report containing counts, unresolved cases, and proposed changes. The adoption UI must make the scope visible before activation.

**Activation boundary:** completing this implementation phase does not itself authorize automatic writes across the real existing sync tree tree. The user can activate the reviewed profile in the app or explicitly instruct us to do so.

## Phase 9 Complete Google Drive workflows

Finish the controls and file policies expected in version 1.

**Deliverables**

- Support multiple accounts and folder pairs with isolated identities, databases or namespaces, credentials, change cursors, and error states. Prevent overlapping local roots.
- Add folder-level selective sync and ignore rules with previews. New excluded content stays outside the reconciliation baseline; existing baselines are retained so changing an exclusion never implies deletion. Deselection keeps local files by default and never implies remote deletion. Default-on .gitignore support and its folder override are implemented; broader selective-sync controls remain pending.
- Complete conflict resolution: keep both, use local, use remote, retry comparison, and defer. Destructive choices retain recovery copies and verify that the displayed versions are still current.
- Define portable filename mapping for case collisions, duplicate Drive names, Unicode, invalid platform characters, and reserved names. Keep mappings reversible and stable across restarts.
- Handle Google Docs, Sheets, and Slides as stable browser link files by default, with a clear policy for existing InSync link files. Identify shortcuts and unsupported items without accidentally following cycles or uploading link files as replacements for native documents. Native document export is a distinct operation from downloading ordinary file bytes. [Drive download and export guide](https://developers.google.com/workspace/drive/api/guides/manage-downloads)
- Respect read-only shared folders and file capabilities; surface permission changes. Team shared-drive support can be added later and is not required for the personal Drive release.

**Complete when** the UI covers the declared version 1 workflows and tests verify account isolation, selection changes, portable mappings, native link files, permission restrictions, and conflict choices against stale versions.

## Phase 10 Recovery scale and performance validation

Exercise sustained and adversarial workloads, building on the safety tests already required in earlier phases.

**Deliverables**

- Test crashes at operation boundaries, disk-full conditions, unreadable files, revoked permissions, root relocation, rate limiting, remote changes during pagination, and lost database state.
- Back up and restore application state, test schema upgrades, and rebuild an inventory without interpreting absent history as deletion authority.
- Measure startup time, scan time, idle CPU, memory, database size, API request counts, and transfer throughput on a recorded representative dataset. Set measured resource budgets and fix regressions before release.
- Avoid repeatedly hashing unchanged files or scanning the entire account. Verify that large trees, many small files, and a long conflict queue remain responsive.
- Add redacted support exports and recovery instructions with actionable error categories.

**Complete when** the fault matrix converges or produces a clear recoverable stop with preserved content; no case silently loses data. Record benchmark measurements, dataset characteristics, soak results, and remaining limitations in the repository.

## Phase 11 Cross-platform validation and installers

Prove that the same application behaves correctly on Windows and macOS as well as Linux.

**Deliverables**

- Configure builds and meaningful automated tests for all three operating systems. Validate native watcher behavior, path handling, file replacement, credential storage, tray menus, and startup at login on each platform.
- Package Linux, Windows, and macOS previews using suitable Tauri distribution formats. Record runtime dependencies, minimum supported OS versions, installation steps, and uninstall behavior. [Tauri distribution](https://v2.tauri.app/distribute/)
- Exercise real account login and a small live sync round trip on each platform. Where direct access is unavailable, supply a concrete test checklist and collect the results rather than claiming runtime verification from a successful build.
- Test upgrading a preview while retaining profiles and state. Uninstall must preserve synchronized user files and clearly explain what happens to app settings and stored authorization.

**Complete when** installable artifacts build for all three systems, clean-machine installation and upgrade checks pass, and platform-specific runtime results are recorded. Missing platform execution remains an explicit unmet acceptance criterion.

**External prerequisite:** access to Windows and macOS runners or test machines, and Google consent on those machines. Signing credentials and notarization accounts may also be needed for the selected distribution channel.

## Phase 12 Release readiness and handover

Deliver version 1 with documented installation, migration, recovery, and maintenance.

**Deliverables**

- Finish onboarding, user documentation, known limitations, account removal, uninstall, and the InSync handover guide. Verify that disconnecting an account leaves user files intact.
- Resolve the OAuth production configuration and verification requirements for the intended audience. Verify the selected release channel's signing and notarization requirements; clearly identify preview artifacts that remain unsigned.
- Implement an update workflow with authenticated release metadata, verified artifacts, and safe schema upgrades. It must support user control over installation and recovery from a failed update.
- Produce a versioned release candidate, checksums, a changelog, and a release checklist. Test installation, sync, update, migration, pause, and recovery end to end.
- Document architecture, build commands, test commands, provider extension points, and ownership of external configuration. Keep release credentials out of source and diagnostics.

**Complete when** every version 1 acceptance criterion has evidence, the intended distribution artifacts are verified, and external prerequisites for that channel are satisfied. Public uploading, publishing, or paid account setup requires an explicit release instruction; local release preparation can finish independently.

**Milestone:** a complete Google Drive desktop sync application for the declared scope. Additional providers are the next roadmap, not a hidden requirement for this milestone.

## How to use this roadmap for implementation goals

A useful first goal is: "Implement FreeSync through Phase 6 in PLAN.md. Use the existing existing sync tree files for read-only inspection and confine live sync tests to test-freesync. Complete each phase's acceptance checks and document the evidence."

Choose Phase 7 for a desktop preview, Phase 8 for adoption readiness, Phase 9 for the version 1 feature set, or Phase 12 for the complete app and release preparation.

During implementation, keep this roadmap's phase numbers stable and maintain `PROGRESS.md` with completed work, commands and results, live verification, and external prerequisites. A phase is complete only after its stated checks pass. Do not substitute a fake provider, a compiled installer, a toast, or an unexamined API response for required live evidence.

Continue independent work when a browser login, platform test, or external account action is pending. Record the missing acceptance check and report it clearly; a pending prerequisite is not a completed phase. Stop at the selected phase after its checks are satisfied, and do not create a goal until Bruno asks to start one.
