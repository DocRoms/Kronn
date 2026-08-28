#!/usr/bin/env python3
import sys
import json
import os

# Minimal renderer to generate .env.example from workflow files
# Extracts ${env:VAR} references from the "env" contract and generates a deterministic list.

def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <workflow.json...>", file=sys.stderr)
        sys.exit(1)

    env_vars = set()
    for fpath in sys.argv[1:]:
        try:
            with open(fpath, 'r') as f:
                data = json.load(f)
        except Exception as e:
            print(f"Error: Invalid JSON in {fpath}: {e}", file=sys.stderr)
            sys.exit(1)
        
        env_section = data.get("env", {})
        if not isinstance(env_section, dict):
            print(f"Error: 'env' section must be an object in {fpath}", file=sys.stderr)
            sys.exit(1)
            
        for k, v in env_section.items():
            if not isinstance(v, str) or not v.startswith("${env:") or not v.endswith("}"):
                print(f"Error: Invalid env reference in {fpath}: {v}. Must be ${{env:VAR}}", file=sys.stderr)
                sys.exit(1)
            var_name = v[6:-1]
            env_vars.add(var_name)
            
    for var in sorted(env_vars):
        print(f"{var}=")

if __name__ == "__main__":
    main()
