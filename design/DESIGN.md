# Desktop preview design

Local ImageGen concepts guided the primary screen and folder dialog. Those private design references are excluded from the public repository. App text, icons, controls and state are implemented in code; no raster UI is shipped in the desktop bundle.

## Extracted system

- True white main/dialog surfaces; pale neutral green sidebar `#f2f5f2`, ink `#172826`, secondary text `#65736c`, border `#e1e7e2`, primary forest green `#26744c`; no gradients, decorative imagery or texture.
- At the 1100 × 760 desktop baseline: 212px sidebar, 30px main gutter, 36px heading, 26px wordmark, 16px section labels, 14px controls/content and 13px supporting text. System humanist sans; medium control labels and semibold headings. Space scale 4/8/12/16/24/32.
- Flat navigation/list rows and open folder details with horizontal rules; no nested cards. Buttons/inputs 6px corners, selected rows 5px, modal 10px, one quiet modal shadow. Controls have minimum 38px height and visible green focus outlines.
- Code-native outline icons: Folder, ArrowLeftRight, Settings, Pause/Play, RefreshCw, Plus, ChevronRight, Eye and Power; 20px, 1.8px rounded strokes. Green filled circular Check status only. Close/X in dialogs, ArrowRight on continuation, ArrowLeft in folder browsing. Typographic FreeSync wordmark, no logo asset.
- Primary layout: rail navigation; page heading + pause/sync controls; Your folders/Add folder; selected pair list; two-column local/Drive paths; Last synced/Pending changes; Preview changes; recovery explanation; running status footer. Rail account and hide/quit stay at the bottom.
- Folder dialog: title/description, account/reconnect, local path with native chooser, Drive breadcrumbs/paginated folder list, selected folder, polling/deletion controls, scope explanation, Cancel/Preview changes. The preview lists actual operations/conflicts/skips and requires explicit activation.
- Below 760px, compact horizontal navigation, wrapped heading controls, single-column details/forms, scrollable main and dialog, footer wraps. Keyboard navigation, focus trapping and Escape restore focus without silently enabling sync. Respect reduced motion.

## Copy lock and functional deviations

Primary fixed copy follows the reference: Folders; Choose what stays in sync.; Pause; Sync now; Your folders; Add folder; Two-way sync; Local folder; Google Drive folder; Last synced; Pending changes; Preview changes; Deleted files are kept in recovery.; Folder settings are saved automatically; Personal Google account; Connected; Hide settings; Quit.

Folder name/path, account email, connection/status labels, counts and timestamps come from real Rust state, replacing sample Up to date/Just now/None as needed. The folder browser contains actual Drive entries rather than the concept's sample Documents/Pictures. Reconnect, error/review banners, Keep both, startup/notification switches, pause-before-review explanation, explicit activation and login/import/cancel controls are required functional states. The detail concept's unrelated background sample rows and altered main-screen copy are ignored; the primary concept controls the app shell.

Preview scope remains the private authorized test pair until Phase 8. Choosing another directory or remote folder returns a readable Rust validation error. No automatic writes across the existing existing sync tree tree are activated.
