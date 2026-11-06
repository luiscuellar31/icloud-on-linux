#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v docker >/dev/null 2>&1; then
    printf 'Docker was not found. Install it to open the lab.\n' >&2
    exit 1
fi

if ! docker ps >/dev/null 2>&1; then
    printf 'Docker is unavailable. Start Docker or Colima and open this file again.\n' >&2
    exit 1
fi

printf 'Preparing the iCloud lab...\n'
docker build --quiet --tag icloud-linux-lab "$project_dir" >/dev/null

exec docker run --rm -it \
    --name icloud-linux-lab \
    --mount type=volume,source=icloud-linux-state,target=/state \
    icloud-linux-lab
