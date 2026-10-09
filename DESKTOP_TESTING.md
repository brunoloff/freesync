# Linux desktop preview verification

These checks use the implemented React interface, the actual Tauri Linux process,
and its sole Rust engine owner. The opt-in debug browser bridge forwards to the
same Rust dispatcher as native IPC. Tests use the private authorized local and
Drive test pair. They do not activate an existing production tree.

## UI and persisted behavior

| Workflow | Verified result |
| --- | --- |
| Login/reconnect | The UI completed real Google consent and the PKCE/loopback callback. A fresh CLI process subsequently read the OS credential store and forced a successful refresh. |
| Configuration | Typed local-folder selection and real paginated Drive folder browsing selected the authorized pair. A different local root returned a readable validation error. Changed polling and deletion limits persisted. |
| Preview/activation | Configuration saved the pair disabled and paused. The UI showed actual operation/conflict counts, then explicitly activated it. Cancelling a later preview resumed an already active pair. |
| Automatic sync and pause/resume | A disposable local change stayed unuploaded while paused; Resume transferred it, with persisted Drive bytes verified. |
| Conflict preservation | A simultaneous-edit fixture appeared in the UI. Keep both preserved the Drive version under the original name and the local version under a conflict name. Both versions were verified on disk and Drive; unrelated retained conflicts remained visible. |
| Window close | Hide settings triggered native `CloseRequested`, which hid the actual window. Native visibility became false while the tray object remained present. A new file transferred successfully while hidden. Show settings restored native visibility. |
| Quit/restart | Choosing Quit stopped the native process with status 0. The supervisor restarted it; the saved pair and preferences survived. The final interface quits directly without an extra confirmation dialog. |
| Single ownership | Starting the CLI engine while the desktop owned the same profile returned `lock_busy`. |
| Native renderer/IPC | An opt-in probe in the actual WebKit frontend confirmed the Folders view mounted, one pair rendered, the account showed Connected and Pause was enabled. The native Tauri command received the report from the main webview; its process-bound evidence recorded a 36px heading and actual viewport geometry. The browser bridge cannot submit this report. |
| Keyboard | Tab and Shift+Tab wrap inside the folder dialog; Escape dismisses it and restores focus to Folder settings. Nested reconnect uses a distinct accessible dialog name; cancelling it restores focus to Reconnect in the folder dialog. A failed first check exposed a focus escape and was fixed before the passing check. |
| Diagnostics | Browser warning/error logs were empty in the final session. Routine engine events contain timestamps, counts and typed error codes, without credentials, private paths or file contents. |

Startup and notification controls are implemented and initially off. They remained
off during these checks. Automatic approval review rejected enabling persistent
startup without a separate user choice. This does not prevent exercising the
required sync workflows.

Native computer automation is unavailable in this environment. The browser tests
therefore do not claim to verify OS tray mouse clicks, native chooser interaction,
notification delivery, startup execution, or a pixel screenshot of WebKit output.
Actual WebKit DOM rendering and native IPC were verified by a flag/count-only,
debug-only report submitted through the native command channel. Native window
lifecycle, tray-object creation, process exit, stored state and real engine
transfers were also verified. The graphical local chooser is implemented; the
typed-path selection route was exercised. OS interaction checks belong on the
platform validation checklist before distribution.

The lifecycle checks were repeated after the watcher correction: UI Quit exited the native process cleanly, and restart preserved the account, active pair, 30-second polling and 25-file deletion limit. Startup and notifications stayed off. The next remote content checkpoint converged while the actual settings window was hidden and the tray object remained present; Show settings restored native visibility.

## Design and rendered inspection

The retained private design references are `design/folders-concept.png` at
1509 × 1042 and `design/pair-concept.png` at 1438 × 1093. They are not shipped UI
assets. The final browser screenshots were captured at those native dimensions.
Both reference/render pairs were opened with `view_image` in the same QA passes.
The ordinary browser viewport, a 620 × 520 minimum-window layout, and a 400 × 800
narrow layout were also inspected. No horizontal content overflow was found;
small windows scroll vertically to the remaining content.

| Comparison point | Reference and rendered evidence | Correction or intentional difference |
| --- | --- | --- |
| Copy | Navigation, Folders heading/subtitle, Pause, Sync now, Your folders, Add folder, field labels, Preview changes and recovery/footer text follow the reference. | Real timestamps, counts and selected identity replace illustrative values. Folder settings, review/activation and error states implement required workflows. No unrelated product copy was added. |
| Layout | The primary screen retains the pale rail, open main surface, selected row, two-column paths, ruled details and bottom footer. | Rail width, main gutters and footer alignment were corrected against the primary reference. The detail concept's unrelated background sample rows are excluded. |
| Typography | Large reference-size heading, wordmark, section label and control scales preserve the reference hierarchy; smaller windows use the documented compact scale. | Heading/control/sidebar sizes were corrected, and the account label was kept on one line at desktop sizes. System font rendering can vary by operating system. |
| Palette and surfaces | White main/dialog surfaces, pale green rail, forest-green primary actions and quiet borders follow the extracted tokens. | No additional gradient, tinted main background or decorative image was introduced. |
| Icons and assets | Outline folder, navigation, settings, pause, refresh, eye, power and chevron icons preserve the reference metaphors and stroke style. | Folder icon sizing and row spacing were corrected. Actual conflict status replaces the sample healthy check mark. Interface text and controls remain code-native. |
| Dialog geometry | The folder dialog retains the reference width, field order, folder browser, two-column options, scope note and right-aligned footer actions. | The first render was too tall; browser height, spacing and control geometry were corrected. The folder browser lists real Drive entries instead of sample folders. |
| Responsive behavior and motion | Below the desktop breakpoint, navigation and forms reflow with readable controls, a scrollable dialog and visible keyboard focus. | 620px and 400px layouts were visually inspected. Reduced-motion rules disable ornamental animation. Explicit focus wrapping corrected a browser focus escape. |

The above-the-fold copy diff found only the documented functional/real-state
differences. The final browser implementation was faithfully verified against
the accepted references, with no remaining material visual mismatch in that
renderer. Private concept and screenshot files are excluded from publication.

## Sustained sync evidence

The automatic live sequence passed creation, edits, stable-identity moves,
recoverable deletion, simultaneous edits, pause/resume, actual network failure,
offline changes, restart and stable empty queues. The first monitored soak was
interrupted when its terminal session ended and is not counted as completed. A
later supervised run verified seven content checkpoints and three clean restarts,
but was stopped after a regression exposed scan feedback from read/access events
and unrelated sibling changes. The watcher correction preserves real native
modifications and rescan warnings. All six live automatic-sequence checks passed
with the corrected runtime.

On 9 October 2026, `scripts/live_soak.py` completed 10,820.76 seconds with the
corrected native desktop owner, independently of terminal session lifetime. All
17 alternating local/remote checkpoints verified actual disk bytes and downloaded
Drive bytes, retained the intentional conflict and ended with an empty queue.
Two scheduled restarts plus the UI Quit/restart were clean. The final Quit returned
status 0; both the owner and supervisor stopped. The private report records
success, completion time and graceful shutdown; elapsed time alone cannot pass.

The normal frontend was subsequently rebuilt without `VITE_FREESYNC_NATIVE_PROBE`,
and the native binary without the `browser-test` feature. The normal Linux preview
started with the saved test pair and no pending transfers. The last 1280 × 720
browser capture showed Connected, the three retained test conflicts, verified
native-renderer/IPC indicators and no horizontal overflow; warning/error logs
were empty. Phases 1–7 are complete within the declared Linux test-pair scope.

## Conflict and preference refinements — 9 October 2026

The follow-up Linux checks exercised the actual native owner through the same UI dispatcher. Use local, Use Drive and Keep both were queued on different rows while another decision was preparing; navigation and the other rows remained usable. Each selected outcome was verified on disk and by a fresh Drive download. Displaced originals remained in recovery, Keep both's second name existed on both sides, the previous unrelated conflict remained unchanged, and the durable transfer queue returned to zero.

Diff launched the installed KDiff3 process with labeled Local and Google Drive inputs. Both temporary files had the exact expected bytes and no write permission bits; synchronized originals stayed unchanged. A binary comparison showed a readable rejection and removed its temporary copies. Refresh comparison updated a conflict without choosing a side. An initial viewer-close check exposed KDiff3's normal status 1 for an unsaved comparison; the correction accepts that status and has a regression test. Other system viewers and operating systems remain unverified.

Ctrl-wheel input changed the displayed/persisted zoom from 100% to 110%. The actual WebKit probe reported the corresponding native viewport geometry, and UI Quit/restart preserved 110%. Ctrl + 0 and the reset control restored 100%. Recovery's Open folder action launched the actual file manager with the private recovery path. Linux's notification service acknowledged the test; inhibition was reported in an earlier desktop read, and was no longer reported during the later test. Notification popup pixels were not observed.

The 1280px and 620px conflict/preferences layouts were visually inspected; controls wrapped, and DOM geometry showed no horizontal overflow. Warning/error console logs were empty before Quit. Existing startup/notification settings were retained. Seven additional core tests and one desktop regression test bring the Rust total to 46. These focused checks do not constitute another three-hour soak or large-tree adoption validation.

## Activity and tray refinements before Phase 8

Checked 2026-10-09 on Linux/KDE against the existing private test-only profile. A private schema-1 backup was saved before upgrading to schema 2. All live fixture mutations stayed in a new disposable run under the authorized local/Drive test roots.

| Check | Result and evidence |
| --- | --- |
| Tray activation | The actual StatusNotifierItem registered with the desktop watcher, exported `ItemIsMenu=false`, and primary `Activate` changed native window visibility true → false → true. The tray remained registered and the engine remained active. |
| Context menu | The exported DBusMenu retained status, Open settings, Pause sync, Resume sync, Sync now and Quit. Actual menu events paused/resumed the engine and graceful Quit exited the process; restart retained history/settings. |
| Navigation | Hide settings and Quit are absent from the GUI. Preferences moved below the account status and remains visible in the desktop sidebar while Activity scrolls. |
| Real activity | Eight uploads, including 0-byte and 2 MiB files, matched local/Drive checksums and recorded exact completed sizes. A 552-byte Drive download matched actual bytes. Trashing only the disposable 50-byte file retained its original bytes in recovery, with a history recovery path. Existing conflicts and preferences were unchanged. |
| Filters/details | Combined name/path, minimum-byte size, Completed outcome and Largest-first sorting showed the six expected files. Expanded upload details displayed actual operation ID, attempt and exact/transferred bytes. The 0-byte/unknown-size distinction, Unicode/literal search, time/action filters, range validation and other sorts have focused backend tests. |
| Pagination | First page displayed 50 real events; Next showed 26 older events with no overlapping IDs and disabled Next. Previous and combined filters returned to the current page. Backend tests also insert new events between anchored page reads. |
| Persistence/logs | History remained after actual tray Quit/restart. The JSON mirror contained unique positive event IDs and mode 0600. The scoped recovery bytes were verified. Open logs folder returned native-opener success. Failed-log retry/symlink refusal and bounded rotation are tested with isolated profiles. |
| Layout/console | Default desktop and the 620 × 520 minimum window layout were visually checked with no horizontal overflow. The desktop sidebar stays visible on long lists. A narrow outcome badge initially wrapped mid-word and was corrected. Browser warning/error logs were empty. |

The UI checks use the real running native dispatcher through the opt-in browser bridge. Native window visibility, tray registration/menu protocol and process shutdown were checked on the actual host. Tray tests call the app's published StatusNotifierItem/DBusMenu methods; they do not claim physical mouse-click or OS menu-pixel automation. Activity rendering/filter interactions were inspected in the browser mirror; Windows/macOS tray interactions are not yet tested. These checks are not a new three-hour soak or large-tree adoption validation. The final normal build has no QA listener.

### Tray foreground correction

On 2026-10-09, replaced the Linux queued focus request with GTK presentation on the main thread and a fresh X11 user timestamp for explicit tray activation. Primary activation now hides only a visible, unminimized, focused window; a background window is raised instead. Native checks via the actual tray service verified `(visible, focused)` states `(true, false) → (true, true) → (false, false) → (true, true)`, with the tray registered throughout. No always-on-top flag or desktop focus-prevention preference was changed. Normal/QA builds and Clippy passed. This validates Linux/X11 focus through the app's published tray protocol, not physical mouse automation.

## Phase 8 adoption checks

Checked 2026-10-09 against the native Linux app and its private browser bridge. The real Crapbox job remains read-only; UI activation was exercised only in a disposable profile rooted inside the authorized test mapping.

| Check | Evidence |
| --- | --- |
| Background inventory | Drive progress advanced independently of normal folder sync. Cancellation saved the current page; resume and a native restart retained checkpoints, including 874,920 items. |
| Inherited exclusions | The UI showed the managed test folder, active development checkout and one exact existing local path recorded outside the matching InSync account/root's selection. No credentials or inferred patterns were imported. |
| Scope review | The dialog showed local root, Drive folder ID, account, proposed counts and protected rules. Activation was disabled until consent. Its full-width layout fit a 620 × 520 viewport without horizontal overflow. |
| Actual GUI activation | The disposable profile installed two equivalent baselines and one mismatch conflict, with zero queued transfers. The folder appeared enabled in Folders and showed its selected Drive scope. |
| Backups | The pre-activation SQLite backup reopened with no pairs; the manifest backup retained all four fixture findings. The receipt bound exactly the disposable selected subfolder. |
| Live provider behavior | A seeded migration retained the unchanged Drive ID and preserved differing bytes. A later new file uploaded and its downloaded bytes matched; writes outside the selected scope were rejected. |
| Cleanup | Original local fixture files were retained in the private fixture profiles; paired Drive fixture roots went to recoverable Trash. Normal test-folder sync was resumed. |
| Browser diagnostics | No warnings or errors appeared during the scope review and activation checks. |
| Busy Drive recovery | Repeated real listings retained an incomplete-scan warning when the change feed advanced. After the local-first correction, Resume started local hashing immediately; 60,160 entries and 18.6 GiB of completed hashes were observed. Folders and Adoption navigation remained responsive while hashing a large video. |
| Adoption activity | Combined Adoption action and Error outcome filters showed the saved incomplete-scan event; expanded details displayed its error code and duration. No browser warning/error logs appeared. |
| Archived dates/checkpoint | The real local inventory stopped on a pre-epoch folder date. Signed timestamp handling passed 64 tests and a fresh read-only scan of the real 26-entry archive. Both private backups passed integrity checks and retained 757,504 cached entries covering 65,731,555,167 bytes. Adoption was stopped at the user's request, then resumed with explicit authorization. The full scan passed the archived folder and checkpointed its negative timestamp, reusing over 602,000 checksums. Browser warning/error logs remained empty. |

Full real-report counts and final runtime evidence are still pending; these fixture checks do not establish Phase 10 full-tree performance acceptance.
