# Adopting an existing synchronized folder

Open **Adoption** in FreeSync. Verify the Google account, local root and My Drive root shown there. The setup mapping pins Drive's opaque root ID; a similarly named folder is not a substitute. Stop InSync before starting FreeSync transfers.

## Read-only inventory

Review exclusions, then select **Start read-only inventory**. Already managed folders and a development checkout inside the source tree are suggested exclusions. Existing local paths recorded outside InSync's sync selection are imported only when its saved account and root match. These are literal paths, not guessed wildcard rules; missing items and cloud tombstones are not imported. Saved InSync credentials are never imported. Review any other old-client ignore rules yourself. This preview uses the root mapping established during OAuth setup.

The inventory hashes ordinary local files first, then lists Drive metadata, resolves Drive ancestry and consumes changes that arrived during scanning. This saves useful local work even if Drive scanning is interrupted. Final local verification rechecks saved stamps and comparisons, including the source directory itself so a new top-level entry cannot be missed. Matching files keep their existing Drive IDs. Duplicate names and initial differences remain unresolved. Native documents, shortcuts, existing InSync link files, symbolic links, unreadable branches and unsupported names remain protected or need review. They are not interpreted as deletion authority.

Changes during Drive pagination can shift its results. FreeSync first lists folders, then checks their children in groups of up to 64 parent folders, with up to four groups in flight. A complete single-page group can merge intervening changes directly; a group with multiple pages must have a quiet change-feed window while its cohort is checked. Any intervening change repeats the current multi-page groups: a moved file can hide its old parent when a peer listing already shows its new location. Other completed cohorts remain saved, rather than repeating the whole account. New folders found through the change feed are checked too. If the folder census or a cohort keeps changing after three attempts, resume after it settles. The interface shows Drive items and checked-folder counts. [Google's listing token guidance](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list)

Temporary metadata-read failures and rate limits get up to five attempts with exponential backoff and jitter. Server retry delays up to 60 seconds are respected; longer delays stop the current attempt rather than being shortened. Cancellation interrupts the wait. Permanent permission, authentication and invalid-page errors remain visible. [Google's error guidance](https://developers.google.com/workspace/drive/api/guides/handle-errors)

**Cancel inventory** saves progress. **Resume inventory** retains verified folder groups and partial pages, and reuses checksums only when the local identity, size and modification time still match. It walks the local tree again and rechecks saved comparisons. A failed concurrent request does not discard its successful peers. Intervening change evidence is saved with its cursor, including interrupted change feeds. An expired child page restarts only its group; an expired folder-census page or change cursor requires a fresh Drive inventory while retaining local checksum checkpoints. A source root replacement stops adoption.

The private profile's `adoption/manifest.sqlite3` contains the full inventory and findings. `adoption/summary.json` contains the completed counts and the first 50 findings; use the UI or CLI filters for the full results. Its WAL is part of live state: do not copy just that database file while the inventory is running. Use the application's consistent backup or stop it before copying the profile. No manifest or report should be committed to a public repository.

The CLI offers the same read-only workflow:

```sh
freesync adopt --exclude 'already-managed-folder'
freesync adoption-report --status unresolved --name 'search text'
```

Ctrl+C cancels a CLI inventory. GUI cancellation controls its own background inventory; a CLI process must be stopped from its own terminal. Neither command starts transfers.

Adoption defaults to respecting local `.gitignore` files. Git-ignored findings are excluded dynamically; they are not copied into permanent literal exclusions. Folder review displays the current included counts and binds activation to the ignore-file/index snapshot, so policy changes require another review. After activation, the folder option can be disabled through a paused preview.

To reapply local rules to an already completed saved inventory without relisting Drive, run `freesync adoption-report --refresh-ignore-rules` while adoption is stopped. This rechecks local files and changes the review revision, but keeps the displayed Drive comparison time. A full comparison still rechecks current Drive metadata before activation.

Report totals count files, folders and other findings. An excluded finding can represent a whole branch. The report is a saved comparison at the displayed completion time; refreshing it makes a new read-only comparison. Proposed uploads/downloads are review findings until a selected scope is activated.

## Review and gradual activation

After the report is ready, search for a folder and choose **Review folder**. Inspect the exact local folder, account, Drive identity, counts of proposed transfers, initial differences and protected paths. Confirm the checkbox and select **Activate reviewed folder** only when you want that folder and its included descendants to synchronize.

Activation rechecks the selected folder and its Drive ancestors against the report. Fresh checksums can confirm equivalent bytes despite metadata-only touches or delayed Drive version updates. Content, location, root or scope changes reject stale activation. Initial mismatches become conflicts; equivalent files become baselines without transfers. Only reviewed uploads and downloads are queued. Existing native link files and known unreadable or unsupported branches are excluded from that scope.

Literal rules under the selected folder and simple global rules such as `**/node_modules` or `*.tmp` carry into its pair. A complex glob spanning its ancestors blocks activation instead of being dropped. Define it with the selected folder's literal source-relative prefix, or select a higher folder; general selective-sync controls are Phase 9 work.

FreeSync saves a consistent profile and manifest backup under `backups/before-adoption-…` before installing the pair. The approval binds the account, local identity, Drive ID and exclusions. Subsequent Google writes check the exact approved root and ancestry. Overlapping pairs are rejected. You can activate another nonoverlapping subfolder later; completing implementation does not activate the existing tree automatically.

Large-tree resource validation remains Phase 10. Begin with a small reviewed subfolder; avoid treating implementation of adoption as evidence that full-root continuous scanning has already met its performance budget.

## Return to InSync

Pause FreeSync, then choose **Quit** from its tray menu. Start InSync only after FreeSync has stopped. Keep both the local tree and its Drive counterpart. Turn off FreeSync's start-at-login option if you intend to keep using InSync.

Keep the private profile, adoption manifest and pre-activation backup. To restore a profile, stop FreeSync, preserve its current profile separately, then copy the backed-up `state.sqlite3` into an empty profile directory and the backed-up `manifest.sqlite3` into that profile's `adoption/` directory. Do not retain WAL/SHM files from a different database version. Before launching the restored profile, run `freesync --profile /absolute/restored/profile pause`; inspect it before resuming. OAuth remains in the OS credential store; it is not included in the database backup.

A backup restores FreeSync's configuration and comparison history. It does not undo transfers performed since the backup. Recovery copies and Drive Trash retain displaced or deleted content according to the normal recovery policy.
