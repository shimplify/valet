#!/bin/bash
set -e

echo "========================================"
echo "Provisioning Shimmer Test Environment"
echo "========================================"

echo "Installing dependencies..."
apt update
apt install -y \
    build-essential \
    pkg-config \
    nvme-cli \
    linux-modules-extra-$(uname -r) \
    zonefs-tools \
    cmake \
    git \
    wget \
    curl \
    libgflags-dev \
    libsnappy-dev \
    zlib1g-dev \
    libbz2-dev \
    liblz4-dev \
    libzstd-dev

echo "Loading null_blk module for ZNS emulation..."
# Remove existing null_blk module if loaded (may have wrong parameters)
if lsmod | grep -q null_blk; then
    echo "Unloading existing null_blk module..."
    modprobe -r null_blk || true
fi

# Load null_blk with zoned parameters
modprobe null_blk memory_backed=1 gb=2 zoned=1 zone_size=128 zone_nr_conv=0

# Verify the zoned device was created
if [ ! -b /dev/nullb0 ]; then
    echo "ERROR: /dev/nullb0 was not created"
    exit 1
fi

# Verify it's actually a zoned device
if ! blkzone report /dev/nullb0 > /dev/null 2>&1; then
    echo "ERROR: /dev/nullb0 is not a zoned block device"
    exit 1
fi

echo "ZNS device /dev/nullb0 created successfully"
echo "Provisioning complete!"
