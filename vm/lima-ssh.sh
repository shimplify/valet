#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_NAME="shimmer"

export LIMA_HOME="$SCRIPT_DIR/.lima"

if ! command -v limactl &>/dev/null; then
    echo "ERROR: Lima is not installed"
    exit 1
fi

if ! limactl list | grep -q "^$VM_NAME.*Running"; then
    echo "ERROR: VM '$VM_NAME' is not running"
    echo ""
    echo "Start it with: ./lima-start.sh start"
    exit 1
fi

if [ $# -gt 0 ]; then
    # Use --workdir to prevent Lima from trying to cd to host path
    limactl shell --workdir / "$VM_NAME" bash -c "$*"
else
    limactl shell "$VM_NAME"
fi
