"""Unit tests for extract_cookies.py that never touch a browser or yt-dlp.

Run with: python3 -m unittest discover -s vendor -p 'test_*.py'

Everything here works on plain strings, so the suite is fast and needs no
browser profile, no network access and no yt-dlp installation.
"""

import json
import time
import unittest

import extract_cookies as ec


def make_cookie(**kwargs):
    """Build a Cookie with sane defaults; kwargs override individual fields."""
    data = {"name": "n", "value": "v"}
    data.update(kwargs)
    return ec.Cookie(**data)


class NetscapeTests(unittest.TestCase):
    """Parsing of Netscape/curl cookie jars."""

    def test_parses_httponly_and_keeps_value_intact(self):
        """The #HttpOnly_ prefix is stripped and values keep their exact bytes."""
        text = (
            "# Netscape HTTP Cookie File\n"
            "#HttpOnly_.example.com\tTRUE\t/\tTRUE\t4102444800\tsid\tv=1\n"
            ".example.com\tFALSE\t/login\tFALSE\t0\tsession\tabc\n"
        )

        first, second = ec.parse_netscape(text)

        self.assertEqual(first.domain, ".example.com")
        self.assertEqual(first.name, "sid")
        self.assertEqual(first.value, "v=1")
        self.assertTrue(first.secure)
        self.assertEqual(first.expires, 4102444800)
        self.assertEqual(second.path, "/login")
        # Expiry 0 means a session cookie, represented as None.
        self.assertIsNone(second.expires)

    def test_skips_comments_malformed_and_empty_values(self):
        """Comments, short rows and empty values are dropped, not raised."""
        text = "# comment\n\ntoo\tfew\n.example.com\tTRUE\t/\tFALSE\t0\tempty\t\n"

        self.assertEqual(ec.parse_netscape(text), [])

    def test_detects_jar_vs_header(self):
        """A jar is recognised by its header row or by having tab-separated columns."""
        self.assertTrue(ec.looks_like_netscape("# Netscape HTTP Cookie File\n"))
        self.assertTrue(ec.looks_like_netscape(".a.com\tTRUE\t/\tFALSE\t0\tx\ty\n"))
        self.assertFalse(ec.looks_like_netscape("a=1; b=2"))


class HeaderTests(unittest.TestCase):
    """Parsing of a bare `Cookie:` header."""

    def test_parses_pairs_and_ignores_broken_parts(self):
        """Pairs without `=` are skipped; an empty value is preserved."""
        cookies = ec.parse_cookie_header("a=1; b=2; broken; c=")

        self.assertEqual(
            [(c.name, c.value) for c in cookies], [("a", "1"), ("b", "2"), ("c", "")]
        )


class DomainTests(unittest.TestCase):
    """Domain filter semantics: exact domain plus subdomains, nothing else."""

    def test_matches_domain_and_subdomains_only(self):
        """`example.com` matches .example.com and .sub.example.com, not the reverse."""
        cookie = make_cookie(domain=".example.com")

        self.assertTrue(ec.domain_matches(cookie, "example.com"))
        self.assertTrue(
            ec.domain_matches(make_cookie(domain=".sub.example.com"), "example.com")
        )
        self.assertFalse(ec.domain_matches(cookie, "sub.example.com"))
        self.assertFalse(ec.domain_matches(cookie, "notexample.com"))
        # Prefix/suffix tricks must not pass the check.
        self.assertFalse(ec.domain_matches(cookie, "example.com.evil.test"))

    def test_any_or_missing_domain_matches(self):
        """No filter, or a cookie with no domain, accepts everything."""
        self.assertTrue(ec.domain_matches(make_cookie(domain=".example.com"), None))
        self.assertTrue(ec.domain_matches(make_cookie(domain=None), "example.com"))


class ExpiryTests(unittest.TestCase):
    """Human-readable, value-free expiry descriptions."""

    def test_labels(self):
        """persistent / expired / session classification."""
        now = int(time.time())

        self.assertEqual(make_cookie(expires=now + 60).expiry_label, "persistent")
        self.assertEqual(make_cookie(expires=now - 60).expiry_label, "expired")
        self.assertEqual(make_cookie().expiry_label, "session")

        self.assertTrue(make_cookie(expires=now - 1).expired)
        self.assertFalse(make_cookie(expires=now + 60).expired)

    def test_remaining_label(self):
        """Days above 24h, hours below, and the two edge labels."""
        now = int(time.time())

        self.assertEqual(
            make_cookie(expires=now + 2 * 86400).remaining_label, "2.0d left"
        )
        self.assertEqual(
            make_cookie(expires=now + 2 * 3600).remaining_label, "2.0h left"
        )
        self.assertEqual(make_cookie(expires=now - 1).remaining_label, "expired")
        self.assertEqual(make_cookie().remaining_label, "session")


class BrowserSpecTests(unittest.TestCase):
    """yt-dlp browser spec parsing: BROWSER[+KEYRING][:PROFILE][::CONTAINER]."""

    def test_plain_browser(self):
        """A bare browser name has no optional selectors."""
        self.assertEqual(ec.parse_browser_spec("chrome"), ("chrome", None, None, None))

    def test_keyring_and_profile(self):
        """`+keyring` and `:profile` are split into separate fields."""
        self.assertEqual(
            ec.parse_browser_spec("chrome+gnomekeyring:Profile 1"),
            ("chrome", "Profile 1", "gnomekeyring", None),
        )

    def test_container_and_windows_profile_path(self):
        """`::container` is last; a Windows drive letter is not a separator."""
        self.assertEqual(
            ec.parse_browser_spec("firefox:/tmp/profile::work"),
            ("firefox", "/tmp/profile", None, "work"),
        )
        self.assertEqual(
            ec.parse_browser_spec(r"firefox:C:\Users\neil\Profile"),
            ("firefox", r"C:\Users\neil\Profile", None, None),
        )


class RenderTests(unittest.TestCase):
    """Output formats produced by render()."""

    def test_json_meta_carries_metadata(self):
        """json-meta must expose domain/path/secure/expires alongside the value."""
        selected = {
            "a": make_cookie(
                name="a",
                value="1",
                domain=".x.com",
                path="/p",
                secure=True,
                expires=123,
            )
        }

        out = json.loads(ec.render(selected, ["a"], "json-meta", ""))

        self.assertEqual(
            out,
            [
                {
                    "name": "a",
                    "value": "1",
                    "domain": ".x.com",
                    "path": "/p",
                    "secure": True,
                    "expires": 123,
                }
            ],
        )

    def test_plain_json_stays_name_value(self):
        """Plain json stays a minimal name → value object."""
        selected = {"a": make_cookie(name="a", value="1")}

        self.assertEqual(json.loads(ec.render(selected, ["a"], "json", "")), {"a": "1"})


if __name__ == "__main__":
    unittest.main()
