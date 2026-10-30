"""Validate Codex releases and check whether their GitHub channel pointers can advance."""

import argparse
import json
import re
import sys

_VERSION_RE = re.compile(
    r"^(?P<upstream>[0-9]+\.[0-9]+\.[0-9]+)"
    r"(?:\.[0-9]+|(?P<prerelease>-(?:alpha(?:\.[0-9]+){0,2}"
    r"|beta(?:\.[0-9]+)?)))?$"
)


def is_valid_release_version(version: str) -> bool:
    return _VERSION_RE.fullmatch(version) is not None


def is_valid_stable_release_version(version: str) -> bool:
    return is_valid_release_version(version) and "-" not in version


def workspace_version_for_release(version: str) -> str:
    """Map a release to Cargo's version, omitting a stable NzSN revision."""
    match = _VERSION_RE.fullmatch(version)
    if match is None:
        raise ValueError(f"invalid release version: {version}")
    return match["upstream"] + (match["prerelease"] or "")


def _version_key(version: str) -> tuple[tuple[int, ...], int, tuple[int, ...], int]:
    base, _, prerelease = version.partition("-")
    parts = tuple(int(part) for part in base.split("."))
    channel, _, suffix = prerelease.partition(".")
    return (
        parts[:3],
        {"alpha": 0, "beta": 1, "": 2}[channel],
        tuple(int(part) for part in suffix.split(".")) if suffix else (),
        parts[3] if len(parts) == 4 else 0,
    )


def should_update_version(version: str, current_version: str) -> bool:
    """Compare version strings; replace an unreadable current version."""
    if not is_valid_release_version(version):
        raise ValueError(f"invalid release version: {version}")
    if not is_valid_release_version(current_version):
        # Replace an unreadable pointer so publishing can repair it.
        return True

    return _version_key(version) > _version_key(current_version)


def should_make_latest(version: str, latest: dict) -> bool:
    if not is_valid_stable_release_version(version):
        raise ValueError(f"expected a stable release version: {version}")
    tag = latest.get("tag_name", "")
    current = tag.removeprefix("rust-v") if isinstance(tag, str) else ""
    if (
        not isinstance(tag, str)
        or not tag.startswith("rust-v")
        or not is_valid_stable_release_version(current)
        or latest.get("draft") is not False
        or latest.get("prerelease") is not False
    ):
        raise ValueError("Cannot determine the current GitHub latest stable release")
    # An equal version must be able to finish a partially completed publication.
    return version == current or should_update_version(version, current)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    checks = parser.add_mutually_exclusive_group()
    checks.add_argument(
        "--check-github",
        choices=("latest", "canary"),
        help="Compare with the latest release JSON or canary Cargo.toml on stdin.",
    )
    checks.add_argument(
        "--workspace-version",
        help="Require the matching Cargo version, excluding any stable NzSN revision.",
    )
    args = parser.parse_args()
    if args.workspace_version is not None:
        try:
            expected = workspace_version_for_release(args.version)
        except ValueError as error:
            parser.error(str(error))
        if args.workspace_version != expected:
            parser.error(
                f"Release {args.version} requires Cargo.toml {expected}, "
                f"got {args.workspace_version}"
            )
        print(f"Release {args.version} matches Cargo.toml {args.workspace_version}")
        raise SystemExit(0)
    if args.check_github is None:
        raise SystemExit(0 if is_valid_release_version(args.version) else 1)
    if args.check_github == "latest":
        update = should_make_latest(args.version, json.load(sys.stdin))
    else:
        import tomllib

        try:
            current_version = tomllib.loads(sys.stdin.read())["workspace"]["package"][
                "version"
            ]
        except (tomllib.TOMLDecodeError, KeyError, TypeError):
            current_version = ""
        if not isinstance(current_version, str):
            current_version = ""
        update = should_update_version(args.version, current_version)
    print(str(update).lower())
