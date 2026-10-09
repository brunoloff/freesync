# FreeSync

A desktop application being developed to synchronize local folders with Google Drive on Linux, Windows and macOS.

The application uses a Rust synchronization engine and a Tauri desktop interface. See [the implementation roadmap](PLAN.md) for the twelve cumulative phases and their acceptance criteria.

Phases 1–7 are complete: a usable Linux desktop preview with local inventory/watchers, native OAuth, a paginated Drive adapter, persistent planning, recoverable transfers and automatic two-way sync within the authorized test pair. The Phase 8 adoption workflow now compares existing trees read-only and activates reviewed subfolders with backups; its full real-tree acceptance report has not yet finished. Validation includes 71 Rust tests, six OAuth-helper tests, live transfer and migration checks, and the earlier three-hour native-desktop run with 17 verified content checkpoints and three clean restarts. Windows/macOS validation and large-tree performance acceptance remain later phases. See [implementation progress](PROGRESS.md), [desktop testing](DESKTOP_TESTING.md), [adoption instructions](ADOPTION.md), [policies](POLICIES.md) and [OAuth setup](OAUTH_SETUP.md).

## Project website

[FreeSync](https://brunoloff.github.io/freesync/), [privacy policy](https://brunoloff.github.io/freesync/privacy.html) and [terms of use](https://brunoloff.github.io/freesync/terms.html). GitHub Pages publishes the `docs/` directory from `main`.

## Rust CLI preview

Build with the host Rust toolchain (the workspace declares Rust 1.90 or later; currently tested with 1.96.1):

```sh
cargo build -p freesync-cli
cargo test --workspace --features freesync-desktop/browser-test
cargo clippy --workspace --all-targets --features freesync-desktop/browser-test -- -D warnings
./target/debug/freesync --help
./target/debug/freesync account --refresh
./target/debug/freesync plan
```

The default profile is outside synchronized content, under the OS application data directory. On Linux it is `~/.local/share/freesync/profiles/default`. It contains private SQLite state, transfer staging and retained recovery files. OAuth credentials remain in the native credential store.

Transfers require either the private, explicitly authorized `test-freesync` mapping or a reviewed adoption receipt. To operate the configured development pair:

```sh
./target/debug/freesync sync-once
./target/debug/freesync activate
./target/debug/freesync run
```

A separate terminal can inspect status or control the single engine owner:

```sh
./target/debug/freesync status
./target/debug/freesync pause
./target/debug/freesync resume
./target/debug/freesync sync-now
./target/debug/freesync quit
```

A stopped large-deletion plan needs an explicit reviewed count and a fresh healthy inventory. Content conflicts remain visible until resolved; retry retains the original expectations, while `keep-both` preserves the local file under a conflict name and reconciles both versions. See command help for these actions. Live testing must stay inside the configured test pair until adoption is separately authorized.

## Desktop preview

On Linux, install the Tauri GTK 3, WebKit2GTK 4.1 and AppIndicator prerequisites described in [Tauri's platform prerequisites](https://v2.tauri.app/start/prerequisites/). Use Node 20.19+ (Node 24 is tested).

```sh
cd desktop
npm ci
npm run build
cd ..
cargo build -p freesync-desktop
./target/debug/freesync-desktop
```

The binary embeds the built frontend and needs no development web server. Rebuild the frontend and binary after UI changes. Close the settings window to keep the engine running in the tray. Left-click the tray icon to raise a hidden or background window, or hide it when already focused; right-click for the menu, including Quit to save progress and stop. The CLI and desktop share an exclusive profile owner; stop one before starting the other. Startup at login is optional and disabled until selected.

The folder dialog browses the connected Drive account, accepts a native folder selection or typed path, saves polling/deletion limits, and opens a paused preview before activation. New development pairs use the private authorized test mapping. Use **Adoption** for the existing-tree mapping established during setup: inventory read-only, inspect discrepancies, then review and activate selected subfolders. Equivalent files retain their Drive IDs; initial differences remain conflicts. Large-folder performance validation remains Phase 10.

Conflict decisions run in the background with per-item progress. Keep both preserves the local file under a conflict name and syncs the Drive version under its original name. Use local retains the Drive original in recovery before uploading; Use Drive retains the local original before replacing it. Changed versions reject stale decisions. Refresh comparison updates the displayed versions without choosing one. Diff validates UTF-8 or UTF-16 text, makes private read-only copies of both versions, and launches an installed system viewer (KDiff3 is verified on Linux). KDiff3 copies are removed when its process closes while FreeSync is running. If FreeSync exits first, or a viewer can hand off to another process, copies remain in the private profile's `comparisons/` directory.

Ctrl + scroll and Ctrl/Cmd + plus/minus zoom the entire interface; Ctrl/Cmd + 0 resets it. Zoom persists between launches. Preferences can open the actual recovery directory in the file manager. On Linux, Test notification reports whether the desktop service accepted the message and whether the desktop reports notification inhibition; suppressed popups are explained in the interface.

For browser UI QA against the **actual native app**, explicitly build the debug-only `browser-test` feature and set `FREESYNC_BROWSER_TEST=1`. Its private profile `browser-test.json` contains a short-lived launch URL. The listener binds a random loopback port, requires a session cookie and same-origin requests, and uses the same narrow Rust dispatcher as native IPC. The optional frontend probe below verifies that the actual WebKit view mounted and reached Rust through Tauri IPC; its private `native-view.json` contains only flags, counts and geometry. The report command is unavailable through the browser bridge. Normal frontend builds omit the probe, and normal Rust builds have neither its command nor the listener. Do not publish the launch URL or the private profile.

```sh
cd desktop
VITE_FREESYNC_NATIVE_PROBE=1 npm run build
cd ..
cargo build -p freesync-desktop --features browser-test
FREESYNC_BROWSER_TEST=1 ./target/debug/freesync-desktop
```

The design references and extracted UI system are in [design/DESIGN.md](design/DESIGN.md). The Activity tab stores local history across restarts, with name/path, file-size, action, outcome and time filters, sorting and pagination. Expand an event for operation IDs, exact sizes, attempts, retries, error codes, scan duration or recovery locations. Open logs folder reveals the private profile: `activity.jsonl` mirrors this detailed history, while `events.jsonl` retains count-only engine summaries. JSON logs rotate after 2 MiB with one previous file; SQLite history is retained without automatic pruning. History begins when this version starts; earlier transfers are not reconstructed. File contents, OAuth credentials and resumable upload URLs are excluded.

## OAuth helper

`scripts/google_oauth.py` supports initial authorization and a read-only verification of saved authorization. It requires Python 3, `google-auth-oauthlib`, `google-auth`, `requests`, `keyring`, and a running, unlocked Secret Service credential store.

Store the desktop client JSON outside any synchronized folder. Never commit or publish client credentials, tokens, authorization codes or private synchronization state.

```sh
python3 -u scripts/google_oauth.py authorize \
  --client-json "${XDG_CONFIG_HOME:-$HOME/.config}/freesync/google-client.json" \
  --account you@example.com

python3 scripts/google_oauth.py verify --account you@example.com
python3 -m unittest discover -s scripts -p 'test_*.py' -v
```

The helper prints an authorization URL for you to open in your browser. Saved user credentials remain in the OS credential store; the local metadata file contains no tokens or client secret.

Live development mutations must remain in an explicitly selected disposable test folder and its paired Drive folder until broader synchronization is enabled by the user.
