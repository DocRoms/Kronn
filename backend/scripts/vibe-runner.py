#!/usr/bin/env python3
"""Kronn Vibe Runner — Direct Mistral API wrapper.

Vibe CLI 2.5+ hangs in programmatic mode (stdin + asyncio issues).
This script calls the Mistral chat completions API directly,
preserving the same interface Kronn expects (text output to stdout).

Usage:
    python3 vibe-runner.py "your prompt here"
    python3 vibe-runner.py --model devstral-small-latest "prompt"

Requires: MISTRAL_API_KEY environment variable.
"""
import argparse
import json
import os
import sys
import urllib.request
import urllib.error

MISTRAL_API_URL = "https://api.mistral.ai/v1/chat/completions"
DEFAULT_MODEL = "mistral-vibe-cli-latest"


def parse_args():
    parser = argparse.ArgumentParser(description="Kronn Vibe Runner")
    parser.add_argument("prompt", help="Prompt to send")
    parser.add_argument("--model", default=DEFAULT_MODEL, help="Mistral model name")
    parser.add_argument("--max-tokens", type=int, default=16384, help="Max output tokens")
    return parser.parse_args()


def load_vibe_env_key():
    """Read MISTRAL_API_KEY from ~/.vibe/.env (same source as Vibe CLI)."""
    env_path = os.path.join(os.path.expanduser("~"), ".vibe", ".env")
    try:
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("MISTRAL_API_KEY="):
                    val = line.split("=", 1)[1].strip().strip("'\"")
                    if val:
                        return val
    except OSError:
        pass
    return None


def main():
    args = parse_args()

    api_key = os.environ.get("MISTRAL_API_KEY", "") or load_vibe_env_key()
    if not api_key:
        print("Error: MISTRAL_API_KEY not set (checked env and ~/.vibe/.env)", file=sys.stderr)
        sys.exit(1)

    payload = json.dumps({
        "model": args.model,
        "messages": [{"role": "user", "content": args.prompt}],
        "max_tokens": args.max_tokens,
        "stream": True,
    }).encode()

    req = urllib.request.Request(
        MISTRAL_API_URL,
        data=payload,
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "Accept": "text/event-stream",
        },
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            for line in resp:
                line = line.decode().strip()
                if not line.startswith("data: "):
                    continue
                data = line[6:]
                if data == "[DONE]":
                    break
                try:
                    chunk = json.loads(data)
                    delta = chunk.get("choices", [{}])[0].get("delta", {})
                    content = delta.get("content", "")
                    if content:
                        sys.stdout.write(content)
                        sys.stdout.flush()
                except json.JSONDecodeError:
                    continue

        sys.stdout.write("\n")
        sys.stdout.flush()

    except urllib.error.HTTPError as e:
        body = e.read().decode()
        print(f"Mistral API error {e.code}: {body}", file=sys.stderr)
        sys.exit(1)
    except Exception as e:
        print(f"Error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
