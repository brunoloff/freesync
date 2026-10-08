# FreeSync

A desktop application being developed to synchronize local folders with Google Drive on Linux, Windows and macOS.

The planned application uses a Rust synchronization engine and a Tauri desktop interface. See [the implementation roadmap](PLAN.md) for the twelve cumulative phases and their acceptance criteria.

Current implementation: a Linux Google OAuth bootstrap helper using PKCE, a loopback callback and the OS Secret Service credential store. Production-mode authorization and refresh have been verified against the personal Google Drive account. See [OAuth setup](OAUTH_SETUP.md). The synchronization engine and desktop interface are still pending.

## Project website

[FreeSync](https://brunoloff.github.io/freesync/), [privacy policy](https://brunoloff.github.io/freesync/privacy.html) and [terms of use](https://brunoloff.github.io/freesync/terms.html). GitHub Pages publishes the `docs/` directory from `main`.

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
