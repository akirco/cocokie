#!/usr/bin/python3
"""Extract only the cookie fields you need, without dumping the whole jar.

Reads cookies either straight from a browser profile (via yt-dlp's cookie
extractor) or from an existing Netscape jar / raw ``Cookie`` header, then
selects cookies **by name** and renders them in the format you want.

A source (``--browser`` or ``--file``) and ``--fields`` are mandatory (except
with ``--list``). Pass ``--fields '*'`` to select every non-expired cookie in
the chosen domain instead of naming them individually.

Examples::

    # check a Spotify login, without printing values
    /usr/bin/python3 tools/extract_cookies.py --browser chrome \\
        --fields sp_dc,sp_key,sp_t --domain spotify.com --check

    # write a 0600 jar for spotapi
    /usr/bin/python3 tools/extract_cookies.py --browser chrome \\
        --fields sp_dc,sp_key,sp_t --domain spotify.com \\
        --out ~/.cache/spotify_cookie

    # discover which names exist for a domain (never prints values)
    /usr/bin/python3 tools/extract_cookies.py --browser chrome \\
        --domain spotify.com --list

    # everything the site currently has, without naming fields
    /usr/bin/python3 tools/extract_cookies.py --browser chrome \\
        --domain spotify.com --fields '*' --format json

    # pick arbitrary fields from an exported jar
    /usr/bin/python3 tools/extract_cookies.py --file cookies.txt \\
        --fields sp_dc,sp_key --format json

    # one bare value, handy for shell pipelines
    /usr/bin/python3 tools/extract_cookies.py --browser chrome \\
        --fields sp_dc --format value

    # a different site / a different browser profile
    /usr/bin/python3 tools/extract_cookies.py --browser chrome \\
        --domain github.com --fields dotcom_user,logged_in --check
    /usr/bin/python3 tools/extract_cookies.py \\
        --browser "firefox:/path/to/profile" --fields sp_dc --check

Safety notes:

* Nothing is written to disk unless ``--out`` is given; that file is created
  with mode 0600.
* ``--check`` and ``--list`` never print cookie values, and no value is ever
  sent to stderr.
* On this machine yt-dlp is importable from ``/usr/bin/python3`` but not from
  the managed ``python3`` on PATH; the script re-execs into a working
  interpreter when needed. Use the shebang interpreter to avoid that hop.

Exit codes: 0 ok, 1 input/runtime error, 2 a required cookie is missing.

Requirements:

* Any mode: Python >= 3.7 (dataclasses, postponed annotations).
* ``--browser`` additionally needs ``yt-dlp`` importable from that interpreter.
  Current yt-dlp (2026.x) declares ``Requires-Python >= 3.11``, so browser mode
  effectively needs Python >= 3.11. ``--file`` mode does not import yt-dlp and
  works on the older interpreter.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import sys
import time
from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any, cast

HTTPONLY_PREFIX = "#HttpOnly_"
DEFAULT_DOMAIN = "any"

# The script itself needs only 3.7 (dataclasses + postponed annotations). Browser
# mode is additionally constrained by yt-dlp's Requires-Python, which is >= 3.11
# for the 2026.x releases. This constant is only used in the error message.
YTDLP_MIN_PYTHON = (3, 11)


@dataclass
class Cookie:
    """A normalised cookie, independent of the source format."""

    name: str
    value: str
    domain: str | None = None
    path: str = "/"
    expires: int | None = None
    secure: bool = False

    @property
    def expired(self) -> bool:
        """Whether a fixed expiry has already passed (session cookies do not)."""
        return self.expires is not None and self.expires <= int(time.time())

    @property
    def expiry_label(self) -> str:
        """Human-readable, value-free expiry description."""
        if self.expires is None:
            return "session"
        return "expired" if self.expired else "persistent"

    @property
    def remaining_label(self) -> str:
        """Value-free remaining lifetime, e.g. ``178.3d left``."""
        if self.expires is None:
            return "session"
        remaining = self.expires - int(time.time())
        if remaining <= 0:
            return "expired"
        if remaining >= 86400:
            return f"{remaining / 86400:.1f}d left"
        return f"{remaining / 3600:.1f}h left"


def ytdlp_cookies_module():
    """Import yt_dlp.cookies, re-execing into a capable interpreter if needed."""
    try:
        from yt_dlp import cookies as module

        return module
    except ImportError:
        pass

    where = f"{sys.executable} (Python {sys.version.split()[0]})"
    if os.environ.get("EXTRACT_COOKIES_REEXEC"):
        sys.exit(
            f"yt-dlp is not importable from {where}. yt-dlp 2026.x needs Python "
            f">= {YTDLP_MIN_PYTHON[0]}.{YTDLP_MIN_PYTHON[1]}; install it for this "
            "interpreter, or point BROWSER_COOKIES_PYTHON at one that has it."
        )
    for candidate in ("/usr/bin/python3", "/usr/bin/python"):
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            env = dict(os.environ, EXTRACT_COOKIES_REEXEC="1")
            os.execve(
                candidate,
                [candidate, os.path.abspath(__file__), *sys.argv[1:]],
                env,
            )
    sys.exit(f"yt-dlp is not importable from {where} (try /usr/bin/python3).")


def parse_browser_spec(spec: str) -> tuple[str, str | None, str | None, str | None]:
    """Split ``BROWSER[+KEYRING][:PROFILE][::CONTAINER]`` (yt-dlp syntax)."""
    container = None
    if "::" in spec:
        spec, container = spec.rsplit("::", 1)
    browser, sep, profile = spec.partition(":")
    keyring = None
    if "+" in browser:
        browser, keyring = browser.split("+", 1)
    return browser.strip(), (profile if sep else None), keyring, container


def looks_like_netscape(text: str) -> bool:
    """Whether the input is a cookie jar rather than a bare Cookie header."""
    return any(
        line.startswith(("# Netscape", HTTPONLY_PREFIX)) or "\t" in line
        for line in text.splitlines()
    )


def parse_netscape(text: str) -> list[Cookie]:
    """Parse a Netscape/curl cookie jar (yt-dlp ``--cookies`` format)."""
    result: list[Cookie] = []
    for raw in text.splitlines():
        line = raw.rstrip()
        if line.startswith(HTTPONLY_PREFIX):
            line = line[len(HTTPONLY_PREFIX) :]
        elif line.startswith("#") or not line.strip():
            continue
        # split(..., 6) keeps any tabs/equals inside the value intact.
        fields = line.split("\t", 6) if "\t" in line else line.split(None, 6)
        if len(fields) != 7:
            continue
        # Column 1 (include_subdomains) carries no information we need.
        domain, path, secure, expires = fields[0], fields[2], fields[3], fields[4]
        name, value = fields[5].strip(), fields[6].strip()
        try:
            expiry = int(expires)
        except ValueError:
            expiry = 0
        if not name or not value:
            continue
        result.append(
            Cookie(
                name=name,
                value=value,
                domain=domain.strip(),
                path=path.strip(),
                expires=expiry or None,
                secure=secure.strip().upper() == "TRUE",
            )
        )
    return result


def parse_cookie_header(text: str) -> list[Cookie]:
    """Parse an already assembled ``name=value; name2=value2`` header."""
    result: list[Cookie] = []
    for part in text.split(";"):
        if "=" not in part:
            continue
        name, value = part.split("=", 1)
        name = name.strip()
        if name:
            # No domain info: domain filtering is skipped for these entries.
            result.append(Cookie(name=name, value=value.strip()))
    return result


def load_from_browser(
    spec: str,
    profile_override: str | None,
    keyring_override: str | None,
    container_override: str | None,
) -> list[Cookie]:
    """Read cookies from a browser profile via yt-dlp, in memory only."""
    module = ytdlp_cookies_module()
    browser, profile, keyring, container = parse_browser_spec(spec)
    profile = profile_override or profile
    keyring = keyring_override or keyring
    container = container_override or container

    # yt-dlp's CLI accepts lower-case keyring names, but its Python layer does
    # `_LinuxKeyring[keyring]`, i.e. it needs the *member name*. Normalise and
    # validate here instead of letting an obscure KeyError surface later.
    if keyring:
        keyring = keyring.upper()
        if keyring not in module.SUPPORTED_KEYRINGS:
            raise ValueError(
                f"unsupported keyring {keyring!r}; choose one of: "
                + ", ".join(module.SUPPORTED_KEYRINGS)
            )

    # `logger` is deliberately omitted: yt-dlp's default `YDLLogger()`
    # satisfies its own _LoggerProtocol (a hand-rolled stub does not) and is
    # quiet without a YoutubeDL instance.
    # Upstream annotates `keyring` as the private `_LinuxKeyring` enum even
    # though the runtime contract is the member-name string above; cast past
    # that inaccurate annotation rather than suppressing type checking.
    jar = module.extract_cookies_from_browser(
        browser,
        profile=profile,
        keyring=cast(Any, keyring),
        container=container,
    )
    result: list[Cookie] = []
    for cookie in jar:
        expiry = cookie.expires
        result.append(
            Cookie(
                name=cookie.name,
                value=cookie.value or "",
                domain=cookie.domain,
                path=cookie.path,
                expires=int(expiry) if expiry else None,
                secure=bool(cookie.secure),
            )
        )
    return result


def load_cookies(args: argparse.Namespace) -> list[Cookie]:
    """Load cookies from a file when given, otherwise from a browser profile."""
    if args.file:
        path = os.path.expanduser(args.file)
        with open(path, encoding="utf-8", errors="replace") as handle:
            text = handle.read()
        return (
            parse_netscape(text) if looks_like_netscape(text) else parse_cookie_header(text)
        )
    # The mutually exclusive group is required=True, so args.browser is set.
    return load_from_browser(args.browser, args.profile, args.keyring, args.container)


def domain_matches(cookie: Cookie, wanted: str | None) -> bool:
    """Match ``spotify.com`` and its subdomains, excluding look-alike hosts."""
    if wanted is None or not cookie.domain:
        return True
    domain = cookie.domain.lstrip(".").lower()
    target = wanted.lstrip(".").lower()
    return domain == target or domain.endswith("." + target)


def render(
    selected: dict[str, Cookie],
    fields: list[str],
    fmt: str,
    env_name: str,
) -> str:
    """Render the selected cookies in one of the supported output formats."""
    pairs = [(name, selected[name].value) for name in fields if name in selected]
    header = "; ".join(f"{name}={value}" for name, value in pairs)
    if fmt == "header":
        return header
    if fmt == "json":
        return json.dumps(dict(pairs), indent=2, ensure_ascii=False)
    if fmt == "json-meta":
        return json.dumps(
            [
                {
                    "name": name,
                    "value": selected[name].value,
                    "domain": selected[name].domain,
                    "path": selected[name].path,
                    "secure": selected[name].secure,
                    "expires": selected[name].expires,
                }
                for name in fields
                if name in selected
            ],
            indent=2,
            ensure_ascii=False,
        )
    if fmt == "value":
        return pairs[0][1] if pairs else ""
    if fmt == "env":
        return f"{env_name}={shlex.quote(header)}"
    raise ValueError(f"unknown format: {fmt}")


def write_private(path: str, text: str) -> None:
    """Write credentials to disk with 0600, even if the file already exists."""
    path = os.path.expanduser(path)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(text if text.endswith("\n") else text + "\n")
    finally:
        try:
            os.chmod(path, 0o600)
        except OSError:
            pass


def build_parser() -> argparse.ArgumentParser:
    """Define the CLI surface; help text doubles as user documentation."""
    parser = argparse.ArgumentParser(
        description="Extract selected cookies by name from a browser or a cookie jar.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="A source (--browser or --file) and --fields are mandatory, except "
        "with --list; --fields '*' means every cookie in --domain. Values are never "
        "printed by --check/--list, never sent to stderr, and only written to disk "
        "with --out (mode 0600).",
    )
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument(
        "--file",
        metavar="PATH",
        help="read a Netscape jar (yt-dlp --cookies FILE) or a raw Cookie header file",
    )
    source.add_argument(
        "--browser",
        metavar="SPEC",
        help="yt-dlp browser spec: BROWSER[+KEYRING][:PROFILE][::CONTAINER]",
    )
    parser.add_argument("--profile", help="override the profile in the browser spec")
    parser.add_argument("--keyring", help="Chromium Linux keyring (basictext, gnomekeyring, ...)")
    parser.add_argument("--container", help="Firefox container name")
    parser.add_argument(
        "--fields",
        metavar="NAMES",
        help="comma-separated cookie names; '*' selects every cookie in --domain "
        "(required unless --list is used)",
    )
    parser.add_argument(
        "--domain",
        default=DEFAULT_DOMAIN,
        metavar="DOMAIN",
        help="only accept this domain and its subdomains; 'any' disables the filter "
        "(default: any)",
    )
    parser.add_argument(
        "--allow-expired", action="store_true", help="do not skip expired cookies"
    )
    parser.add_argument(
        "--require",
        metavar="NAMES",
        help="comma-separated names that must be present (default: none; "
        "missing required names exit with code 2)",
    )
    parser.add_argument(
        "--format",
        choices=("header", "json", "json-meta", "value", "env"),
        default="header",
        help="output shape (default: header); json-meta adds domain/path/secure/expires",
    )
    parser.add_argument(
        "--env-name", default="SPOTIFY_COOKIE", help="variable name for --format env"
    )
    parser.add_argument("--out", metavar="PATH", help="write to this file with mode 0600")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check", action="store_true", help="report field status without printing values"
    )
    mode.add_argument(
        "--list", action="store_true", help="list available cookie names without values"
    )
    return parser


def main(argv: Iterable[str] | None = None) -> int:
    """Entry point; returns the process exit code documented at the top."""
    parser = build_parser()
    args = parser.parse_args(argv)

    fields = [name.strip() for name in (args.fields or "").split(",") if name.strip()]
    if not fields and not args.list:
        parser.error("--fields must contain at least one name (or '*')")
    all_fields = fields == ["*"]
    if "*" in fields and not all_fields:
        parser.error("'*' must be the only --fields entry")
    if args.format == "value" and (all_fields or len(fields) != 1):
        parser.error("--format value requires exactly one --fields name")

    try:
        cookies = load_cookies(args)
    except (OSError, ValueError, RuntimeError, FileNotFoundError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    domain = None if args.domain.lower() in ("any", "*", "") else args.domain
    pool = [
        cookie
        for cookie in cookies
        if domain_matches(cookie, domain) and (args.allow_expired or not cookie.expired)
    ]

    if args.list:
        # Machine-readable: name / domain / expires (Unix seconds, "-" for
        # session cookies), so callers can compute TTL themselves.
        for cookie in sorted(pool, key=lambda item: (item.name, item.domain or "")):
            expires = "-" if cookie.expires is None else str(cookie.expires)
            print(f"{cookie.name}\t{cookie.domain or '-'}\t{expires}")
        return 0

    if all_fields:
        # Cookies with an empty value are skipped during selection, so exclude
        # them here too and avoid reporting them as NOT FOUND later.
        fields = sorted({cookie.name for cookie in pool if cookie.value})
        if not fields:
            print(
                f"warning: no cookies found (domain={args.domain})", file=sys.stderr
            )

    selected: dict[str, Cookie] = {}
    for name in fields:
        hit = next(
            (cookie for cookie in pool if cookie.name == name and cookie.value), None
        )
        if hit is not None:
            selected[name] = hit

    missing = [name for name in fields if name not in selected]
    required = [name.strip() for name in (args.require or "").split(",") if name.strip()]
    missing_required = [name for name in required if name not in selected]

    if args.check:
        for name in fields:
            cookie = selected.get(name)
            status = "ok" if cookie else "MISSING"
            detail = (
                f"{cookie.domain or '-'} / {cookie.remaining_label}" if cookie else ""
            )
            print(f"{name:24s} {status:8s} {detail}")
        if missing_required:
            print(
                "required cookie(s) missing: " + ", ".join(missing_required),
                file=sys.stderr,
            )
            return 2
        return 0

    for name in missing:
        print(
            f"warning: cookie {name!r} not found (domain={args.domain})",
            file=sys.stderr,
        )
    if missing_required:
        print(
            "error: required cookie(s) missing: " + ", ".join(missing_required),
            file=sys.stderr,
        )
        print("log in to the site in that browser, then retry", file=sys.stderr)
        return 2
    if args.format == "value" and not selected:
        parser.error("no cookie found for --format value")

    text = render(selected, fields, args.format, args.env_name)
    if args.out:
        write_private(args.out, text)
        print(f"wrote {len(selected)} cookie(s) to {args.out} (mode 0600)", file=sys.stderr)
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())