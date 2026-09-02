#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
    echo "usage: scripts/run_nvidia_probe.sh MODEL KEY_NAME [PROFILE]" >&2
    exit 2
fi

model="$1"
key_name="$2"
profile="${3:-smoke}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
env_file="$repo_root/.env"
binary="$repo_root/target/debug/ai-code-constructor"
api_key=""

if [[ ! -f "$env_file" ]]; then
    echo ".env not found" >&2
    exit 1
fi

while IFS='=' read -r name value; do
    name="${name%$'\r'}"
    if [[ "$name" == "$key_name" ]]; then
        api_key="${value%$'\r'}"
        break
    fi
done < "$env_file"

if (( ${#api_key} >= 2 )); then
    first_char="${api_key:0:1}"
    last_char="${api_key: -1}"
    if [[ ( "$first_char" == '"' && "$last_char" == '"' ) ||
          ( "$first_char" == "'" && "$last_char" == "'" ) ]]; then
        api_key="${api_key:1:${#api_key}-2}"
    fi
fi

if [[ "$api_key" != nvapi-* ]]; then
    echo "requested NVIDIA key does not match the nvapi-* format" >&2
    exit 1
fi

if [[ -z "$api_key" ]]; then
    echo "requested NVIDIA key is missing or empty" >&2
    exit 1
fi

if [[ ! -x "$binary" ]]; then
    echo "probe binary not built; run cargo build first" >&2
    exit 1
fi

export NVIDIA_API_KEY="$api_key"
exec "$binary" model-compatibility-probe \
    --model "$model" \
    --profile "$profile" \
    --max-calls 32 \
    --timeout-ms 120000 \
    --checkpoint-dir "$repo_root/.model-probe-state" \
    --pacing-ms 2000 \
    --max-recoveries 3 \
    --max-retry-after-ms 120000 \
    --max-cumulative-wait-ms 300000 \
    --fallback-wait-ms 5000 \
    --ack-live \
    --json
