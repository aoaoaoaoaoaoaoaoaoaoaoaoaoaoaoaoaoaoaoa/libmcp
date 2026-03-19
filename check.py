#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
import tomllib
from pathlib import Path


ROOT = Path(__file__).resolve().parent
WORKSPACE_MANIFEST = ROOT / "Cargo.toml"


def load_commands() -> dict[str, list[str]]:
    workspace = tomllib.loads(WORKSPACE_MANIFEST.read_text(encoding="utf-8"))
    metadata = workspace["workspace"]["metadata"]["rust-starter"]
    commands: dict[str, list[str]] = {}
    for key in ("format_command", "clippy_command", "test_command", "doc_command", "fix_command"):
        value = metadata.get(key)
        if isinstance(value, list) and value and all(isinstance(part, str) for part in value):
            commands[key] = value
    return commands


def run(name: str, argv: list[str]) -> None:
    print(f"[check] {name}: {' '.join(argv)}", flush=True)
    proc = subprocess.run(argv, cwd=ROOT, check=False)
    if proc.returncode != 0:
        raise SystemExit(proc.returncode)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Thin libmcp check runner")
    parser.add_argument(
        "mode",
        nargs="?",
        choices=("check", "deep", "fix"),
        default="check",
        help="Run the fast gate, include docs for the deep gate, or run the fix command.",
    )
    return parser.parse_args()


def main() -> None:
    commands = load_commands()
    args = parse_args()

    if args.mode == "fix":
        run("fix", commands["fix_command"])
        return

    run("fmt", commands["format_command"])
    run("clippy", commands["clippy_command"])
    run("test", commands["test_command"])

    if args.mode == "deep" and "doc_command" in commands:
        run("doc", commands["doc_command"])


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(130)
