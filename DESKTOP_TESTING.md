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
| Keyboard | Tab and Shift+Tab wrap inside the folder dialog; Escape dismisses it and restores focus to Folder settings. Nested reconnect uses a distinct accessible dialog name; cancelling it restores focus to Reconnect in the folder dialog. A failed first check exposed a focus escape and was fixed before the passing check. |
| Diagnostics | Browser warning/error logs were empty in the final session. Routine engine events contain timestamps, counts and typed error codes, without credentials, private paths or file contents. |

Startup and notification controls are implemented and initially off. They remained
off during these checks. Automatic approval review rejected enabling persistent
startup without a separate user choice. This does not prevent exercising the
required sync workflows.

Native computer automation is unavailable in this environment. The browser tests
therefore do not claim to verify OS tray mouse clicks, native chooser interaction,
notification delivery, startup execution, or the WebKit renderer's visual output.
Native window lifecycle, tray-object creation, process exit, stored state and real
engine transfers were verified. The graphical local chooser is implemented; the
typed-path selection route was exercised. OS interaction checks belong on the
platform validation checklist before distribution.

## Design and rendered inspection

The retained private primary and folder-dialog design references are
1509 × 1042 and 1438 × 1093 respectively. They are not shipped UI
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
interrupted when its terminal session ended and is not counted as completed.

`scripts/live_soak.py` supervises a fresh three-hour run independently of terminal
session lifetime. It alternates local and remote changes, downloads actual Drive
content for every checkpoint, verifies retained conflicts and empty queues, and
performs two scheduled native-process restarts plus the UI Quit restart. Private
evidence records heartbeat, owner process, checkpoint results, clean restarts and
the final graceful Quit. A cumulative Phase 7 milestone remains pending until
this evidence reports success; elapsed time alone cannot satisfy it.
