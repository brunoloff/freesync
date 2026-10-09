# Google Drive OAuth setup

OAuth prerequisites and the Rust Phase 3 adapter were completed and verified on 8 October 2026. Phases 1–7, including live transfers and the Linux desktop preview, completed validation on 9 October 2026; see [implementation progress](PROGRESS.md). Public-app verification remains deferred for this personal development setup.

## Google Cloud configuration

- Project: `your-google-cloud-project` (FreeSync).
- Google Drive API enabled.
- External OAuth audience; publishing status **In production**.
- Desktop app client: FreeSync Linux. The active public client ID is recorded in the private local metadata file.
- Requested scope: `https://www.googleapis.com/auth/drive`. This permits working with existing Drive files and is a restricted Google scope.
- Home: <https://brunoloff.github.io/freesync/>.
- Privacy: <https://brunoloff.github.io/freesync/privacy.html>.
- Terms: <https://brunoloff.github.io/freesync/terms.html>.
- Authorized domain: `brunoloff.github.io`.

The project is used personally and has not undergone Google's public-app verification. An unverified-app warning and the unverified user cap apply. Public distribution will need a separate review of Google's verification requirements. [Personal-use exception](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification#exceptions_to_verification_requirements)

Production mode avoids the seven-day refresh-token expiry imposed by External apps in Testing. Access tokens still expire and must be refreshed. Refresh tokens may still be revoked, expire through inactivity or fail for other documented reasons; they are not guaranteed permanent. [Token expiration](https://developers.google.com/identity/protocols/oauth2#expiration)

## Local credentials

The configuration directory is `${XDG_CONFIG_HOME:-$HOME/.config}/freesync`, outside the synchronized tree. On the development host it is `~/.config/freesync`.

- `google-client.json`: private desktop-client configuration, mode `0600`.
- `oauth-bootstrap.json`: non-secret account, public client ID, project, scope and credential lookup metadata, mode `0600`.
- Directory permissions: `0700`.
- User credentials: Linux OS Secret Service, using Python keyring's explicit Secret Service backend. No plaintext refresh-token fallback.
- Keyring service: `freesync.google-drive`; username: the authorized account's email address from local metadata.
- Stored credential value: Google authorized-user JSON, including refresh token, client information and access-token expiry. Do not print or copy it into the repository, logs or synchronized folders.

Read `oauth-bootstrap.json` for the selected account. Do not retrieve or print the keyring value just to check that it exists. `scripts/google_oauth.py verify --account ACCOUNT` refreshes authorization and prints only safe verification results.

The Python bootstrap and Rust implementation use PKCE, state validation, an IPv4 loopback callback on a random port, explicit offline access and account matching. Callback logs and external exception details are suppressed. Open the authorization URL in a normal browser. Rust uses Linux Secret Service, with native credential-store adapters for macOS and Windows awaiting Phase 11 platform validation. [Desktop OAuth guidance](https://developers.google.com/identity/protocols/oauth2/native-app)

## Verified results

- Consent completed for the intended account, confirmed with Drive `about.user.emailAddress`.
- Refresh token saved in Secret Service and read back by the helper.
- Forced access-token refresh succeeded during authorization and again in a fresh process.
- Drive identity and `changes/startPageToken` requests succeeded after refresh.
- Six offline tests passed, covering incorrect client type, unexpected endpoints, account mismatch, missing refresh tokens, private metadata without secrets and exception redaction.
- Public source files were checked for credential patterns before publication.

The original OAuth checks were read-only. Rust sign-in and forced refresh were subsequently verified in fresh processes. The local/remote `test-freesync` pair and separate read-only existing-tree mapping are now recorded in private `development.json`. Integration mutations remain restricted to the designated test pair; the existing synchronized tree has not been adopted.

## Repository and site

Repository: <https://github.com/brunoloff/freesync>. GitHub Pages serves `docs/` from `main`.

The current workspace contains a protected, empty `.git` directory rather than an initialized checkout. Publication uses a separate temporary Git checkout and copies only an explicit list of source files. The workspace metadata is preserved. Credentials and private test evidence are excluded from published files.

Google Cloud currently contains an earlier unused desktop client with the same display name, created during the console's error/retry sequence. Authorization uses the client ID from local metadata; do not select a client by display name alone. The unused client has not been authorized by the bootstrap.
