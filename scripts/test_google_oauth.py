"""Offline checks for credential storage boundaries and account validation."""

import argparse
from contextlib import redirect_stderr
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

import google_oauth as oauth


class OAuthSetupTests(unittest.TestCase):
    def test_web_client_rejected_before_authorization(self):
        with tempfile.TemporaryDirectory() as temporary:
            client = Path(temporary) / "client.json"
            client.write_text(json.dumps({"web": {"client_id": "example"}}))
            with patch.object(oauth.InstalledAppFlow, "from_client_config") as flow:
                with self.assertRaises(oauth.SetupError):
                    oauth.authorize(argparse.Namespace(client_json=str(client), account="user@example.com"))
                flow.assert_not_called()

    def test_non_google_endpoint_rejected_before_authorization(self):
        with tempfile.TemporaryDirectory() as temporary:
            client = Path(temporary) / "client.json"
            client.write_text(json.dumps({"installed": {
                "auth_uri": "https://example.com/authorize",
                "token_uri": "https://oauth2.googleapis.com/token",
            }}))
            with patch.object(oauth.InstalledAppFlow, "from_client_config") as flow:
                with self.assertRaises(oauth.SetupError):
                    oauth.authorize(argparse.Namespace(client_json=str(client), account="user@example.com"))
                flow.assert_not_called()

    def test_account_mismatch_rejected(self):
        with patch.object(oauth, "google_identity", return_value={"emailAddress": "other@example.com"}):
            with self.assertRaises(oauth.SetupError):
                oauth.verify_account(Mock(), "user@example.com")

    def test_missing_refresh_token_is_not_saved(self):
        credentials = Mock(refresh_token=None)
        with patch.object(oauth, "credential_store") as store:
            with self.assertRaises(oauth.SetupError):
                oauth.save_credentials(credentials, "user@example.com", "example-project")
            store.assert_not_called()

    def test_metadata_contains_no_tokens_and_is_private(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "freesync"
            filename = directory / "oauth-bootstrap.json"
            credentials = Mock(refresh_token="fake-refresh", client_id="example-client")
            credentials.to_json.return_value = json.dumps({"refresh_token": "fake-refresh", "token": "fake-access"})
            with patch.object(oauth, "CONFIG_DIR", directory), patch.object(oauth, "CONFIG_FILE", filename), patch.object(oauth, "credential_store") as store:
                oauth.save_credentials(credentials, "user@example.com", "example-project")
                store.return_value.set_password.assert_called_once()
                content = filename.read_text()
                self.assertNotIn("fake-refresh", content)
                self.assertNotIn("fake-access", content)
                self.assertEqual(filename.stat().st_mode & 0o777, 0o600)
                self.assertEqual(directory.stat().st_mode & 0o777, 0o700)

    def test_external_exception_details_are_redacted(self):
        capture = io.StringIO()
        with patch("sys.argv", ["google_oauth.py", "verify", "--account", "user@example.com"]), patch.object(oauth, "verify", side_effect=RuntimeError("fake-secret-token")), redirect_stderr(capture):
            self.assertEqual(oauth.main(), 1)
        self.assertNotIn("fake-secret-token", capture.getvalue())
        self.assertIn("RuntimeError", capture.getvalue())


if __name__ == "__main__":
    unittest.main()
