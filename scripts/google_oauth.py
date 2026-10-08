#!/usr/bin/env python3
"""Bootstrap Google Drive OAuth without saving tokens in the synced repository."""

from __future__ import annotations

import argparse
import json
import logging
import os
from pathlib import Path
import sys

import requests
from google.auth.transport.requests import Request
from google.oauth2.credentials import Credentials
from google_auth_oauthlib.flow import InstalledAppFlow
from keyring.backends.SecretService import Keyring


SCOPES = ["https://www.googleapis.com/auth/drive"]
SERVICE = "freesync.google-drive"
CONFIG_DIR = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "freesync"
CONFIG_FILE = CONFIG_DIR / "oauth-bootstrap.json"


class SetupError(Exception):
    """A deliberately safe explanation that never includes credential values."""


def credential_store() -> Keyring:
    """Use Secret Service explicitly, with no plaintext backend fallback."""
    return Keyring()


def save_credentials(creds: Credentials, account: str, project_id: str | None) -> None:
    if not creds.refresh_token:
        raise SetupError("Google did not return a refresh token; consent must be repeated.")
    credential_store().set_password(SERVICE, account, creds.to_json())
    CONFIG_DIR.mkdir(mode=0o700, parents=True, exist_ok=True)
    os.chmod(CONFIG_DIR, 0o700)
    metadata = {
        "schema_version": 1,
        "account": account,
        "project_id": project_id,
        "client_id": creds.client_id,
        "scopes": SCOPES,
        "credential_service": SERVICE,
        "credential_username": account,
        "credential_backend": "org.freedesktop.secrets",
    }
    temporary = CONFIG_DIR / ".oauth-bootstrap.json.tmp"
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        json.dump(metadata, handle, indent=2)
        handle.write("\n")
    os.replace(temporary, CONFIG_FILE)


def load_credentials(account: str) -> Credentials:
    value = credential_store().get_password(SERVICE, account)
    if not value:
        raise SetupError("No saved FreeSync authorization for the selected account.")
    return Credentials.from_authorized_user_info(json.loads(value), SCOPES)


def google_identity(creds: Credentials) -> dict:
    response = requests.get(
        "https://www.googleapis.com/drive/v3/about",
        params={"fields": "user(emailAddress,displayName,permissionId)"},
        headers={"Authorization": "Bearer " + creds.token},
        timeout=30,
    )
    if response.status_code != 200:
        raise SetupError("Drive identity check failed with HTTP " + str(response.status_code))
    return response.json()["user"]


def verify_account(creds: Credentials, expected_account: str) -> dict:
    identity = google_identity(creds)
    if identity.get("emailAddress", "").casefold() != expected_account.casefold():
        raise SetupError("The authorized Google account does not match the intended account.")
    return identity


def authorize(args: argparse.Namespace) -> None:
    client_path = Path(args.client_json).expanduser().resolve()
    raw = json.loads(client_path.read_text(encoding="utf-8"))
    client = raw.get("installed")
    if not client:
        raise SetupError("Choose a Desktop app OAuth client, not a Web application client.")
    if client.get("auth_uri") != "https://accounts.google.com/o/oauth2/auth":
        raise SetupError("Unexpected Google authorization endpoint in client configuration.")
    if client.get("token_uri") != "https://oauth2.googleapis.com/token":
        raise SetupError("Unexpected Google token endpoint in client configuration.")
    flow = InstalledAppFlow.from_client_config(raw, SCOPES, autogenerate_code_verifier=True)
    # Never log the OAuth callback URL or token exchange bodies.
    logging.disable(logging.CRITICAL)
    print("Waiting for Google consent. Tokens will be stored in the OS credential store.", flush=True)
    creds = flow.run_local_server(
        host="127.0.0.1",
        port=0,
        open_browser=False,
        timeout_seconds=1800,
        authorization_prompt_message="FreeSync authorization URL: {url}",
        success_message="Google consent received. FreeSync is verifying and saving authorization. You may close this tab.",
        access_type="offline",
        prompt="consent",
        login_hint=args.account,
    )
    identity = verify_account(creds, args.account)
    save_credentials(creds, identity["emailAddress"], client.get("project_id"))
    print("Consent saved for the intended Google account.", flush=True)
    verify(argparse.Namespace(account=args.account))


def verify(args: argparse.Namespace) -> None:
    creds = load_credentials(args.account)
    # A forced refresh proves this can work after the initial access token expires.
    creds.refresh(Request())
    identity = verify_account(creds, args.account)
    metadata = json.loads(CONFIG_FILE.read_text(encoding="utf-8"))
    save_credentials(creds, identity["emailAddress"], metadata.get("project_id"))
    response = requests.get(
        "https://www.googleapis.com/drive/v3/changes/startPageToken",
        headers={"Authorization": "Bearer " + creds.token},
        timeout=30,
    )
    if response.status_code != 200 or not response.json().get("startPageToken"):
        raise SetupError("Drive change-feed verification failed with HTTP " + str(response.status_code))
    print(json.dumps({
        "account_matches": True,
        "refresh_verified": True,
        "drive_identity_verified": True,
        "change_feed_verified": True,
        "credential_backend": "OS Secret Service",
        "metadata_path": str(CONFIG_FILE),
    }), flush=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    auth = commands.add_parser("authorize", help="Authorize a Desktop app client and save credentials securely")
    auth.add_argument("--client-json", required=True)
    auth.add_argument("--account", required=True)
    auth.set_defaults(run=authorize)
    check = commands.add_parser("verify", help="Refresh saved authorization and check read-only Drive access")
    check.add_argument("--account", required=True)
    check.set_defaults(run=verify)
    args = parser.parse_args()
    try:
        args.run(args)
        return 0
    except KeyboardInterrupt:
        print("Authorization cancelled.", file=sys.stderr)
        return 130
    except Exception as error:
        # Exception messages from OAuth libraries can contain codes or token details.
        # Report local errors we authored, and otherwise just the exception type.
        detail = str(error) if isinstance(error, SetupError) else type(error).__name__
        print("FreeSync OAuth setup failed: " + detail, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
