#!/usr/bin/env bash
# Print the docker-compose volume lines that hide the host's secrets from the
# backend container.
#
# `$HOME` is mounted read-only at /host-home and the container runs with the
# user's UID, so a 0600 file there is readable by every agent Kronn starts.
# Each listed path is covered by an empty read-only mount. Only paths that exist
# are masked: Docker cannot create a mount point inside the read-only
# /host-home, and an absent path has nothing to hide.
#
# Usage: docker-secret-masks.sh [home] [empty-dir] [empty-file]
set -euo pipefail

home="${1:-$HOME}"
empty_dir="${2:-./.docker/empty}"
empty_file="${3:-./.docker/empty-file}"

# Directories holding credentials nothing in the container reads through
# /host-home. SSH keys are mounted separately under /home/kronn, so their copy
# here is masked too. Not masked, because a Kronn feature reads them here:
# `.azure` (Microsoft 365, backend/scripts/azure-docker-wrapper.sh) and the
# Fastly config (backend/scripts/fastly-docker-wrapper.sh). An agent in the
# same container can still read those; only one container per agent fixes it.
secret_dirs=(
    .config/kronn
    .ssh
    .aws
    .config/gcloud
    .config/gh
    .docker
    .kube
    .gnupg
    .password-store
)
secret_files=(
    .netrc
    .git-credentials
    .npmrc
    .pypirc
    .pgpass
    .config/hub
)

for rel in "${secret_dirs[@]}"; do
    if [[ -d "$home/$rel" ]]; then
        printf '      - %s:/host-home/%s:ro\n' "$empty_dir" "$rel"
    fi
done
for rel in "${secret_files[@]}"; do
    if [[ -f "$home/$rel" ]]; then
        printf '      - %s:/host-home/%s:ro\n' "$empty_file" "$rel"
    fi
done
