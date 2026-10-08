# Initial synchronization policies

The preview includes ordinary hidden files. Exclusion globs match relative paths and apply recursively to excluded directories. Excluded contents are not deletion authority. Symbolic links, special files, non-UTF-8 names and names containing path separators or colons are skipped or reported as unsupported; they are not followed or silently renamed.

Local roots must be real directories and must not overlap another pair or application state. A scan error, unavailable root, changed root identity or incomplete Drive listing blocks reconciliation. Drive duplicate names remain explicit ambiguities. File equality uses content checksums and sizes; timestamps alone do not establish a baseline.

Filesystem events request a rescan. Native watching uses a short bounded debounce; periodic reconciliation also runs when no event arrives. Native watcher failures fall back to scans. A missing root is not an empty inventory. Parent monitoring and periodic scans detect its return, after which root identity must still match the established pair.

Normal synchronization uses Drive trash and retains local deleted/replaced contents in profile recovery storage. It never permanently deletes Drive files. The retention policy for the initial preview is to keep local recovery copies until the user removes them; no automatic cleanup is enabled.

The initial preview requires sync, staging and recovery storage on the same filesystem for local replacements/deletions. Cross-filesystem moves fail visibly before removing the source. Local installation creates a file only if the destination is absent; an intervening editor save is preserved. A failed transfer's staged source/download remains available for recovery. Conflicts remain visible until explicitly resolved, while unrelated paths continue syncing.

Automated development writes are limited to the explicitly mapped `test-freesync` local and Drive roots. The existing InSync tree is available only for read-only inspection through Phase 7. Google-native documents and shortcuts are detected; complete link-file behavior and portable filename mapping are later Phase 9 work.

Operational logs in the private profile contain only authored event names, queue/conflict counts, timestamps and typed error codes. They exclude paths, IDs, file content, credential values and upload-session URLs. Logs rotate after 2 MiB, retaining one previous file.
