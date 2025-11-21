#!/bin/bash
set -e

echo "Installing RocksDB db_bench..."
if command -v db_bench &> /dev/null; then
    echo "RocksDB db_bench already installed"
    exit 0
fi

apt install -y rocksdb-tools

echo "RocksDB db_bench installed"
which db_bench
