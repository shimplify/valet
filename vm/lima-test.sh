#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_NAME="shimmer"
SSH_HELPER="$SCRIPT_DIR/lima-ssh.sh"

export LIMA_HOME="$SCRIPT_DIR/.lima"

FEATURES=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --features)
            FEATURES="$2"
            echo "Building with features: $FEATURES"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--features FEATURE1,FEATURE2,...]"
            exit 1
            ;;
    esac
done

GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

print_header() {
    echo ""
    echo "========================================="
    echo "$1"
    echo "========================================="
}

print_success() {
    echo -e "${GREEN}✓${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

print_header "Running Shimmer Tests in Lima VM"

if ! limactl list | grep -q "^$VM_NAME.*Running"; then
    print_error "VM is not running"
    echo ""
    echo "Start it with: ./lima-start.sh start"
    exit 1
fi

echo "Detecting shimmer path in VM..."
# valet is mounted at /mnt/valet in the VM
SHIMMER_PATH="/mnt/valet"

echo "Checking if shimmer source is accessible at $SHIMMER_PATH..."
if ! "$SSH_HELPER" "test -d $SHIMMER_PATH && test -f $SHIMMER_PATH/Cargo.toml" 2>/dev/null; then
    print_error "Shimmer source not found at $SHIMMER_PATH in VM"
    echo ""
    echo "Expected Cargo.toml at: $SHIMMER_PATH/Cargo.toml"
    exit 1
fi
echo "Shimmer source found at $SHIMMER_PATH"

if [ -n "$FEATURES" ]; then
    echo "Building Shimmer in release mode with features: $FEATURES (this may take a few minutes)..."
    "$SSH_HELPER" "cd $SHIMMER_PATH && cargo build --release --features $FEATURES" || {
        echo "ERROR: Build failed"
        exit 1
    }
else
    echo "Building Shimmer in release mode (this may take a few minutes)..."
    "$SSH_HELPER" "cd $SHIMMER_PATH && cargo build --release" || {
        echo "ERROR: Build failed"
        exit 1
    }
fi

# Check if db_bench is available
echo "Checking for db_bench..."
if ! "$SSH_HELPER" "command -v db_bench" &>/dev/null; then
    print_error "db_bench not found. VM may need reprovisioning."
    echo ""
    echo "Try: ./lima-start.sh delete && ./lima-start.sh start"
    exit 1
fi

TEST_SCRIPT=$(
    cat <<EOF
#!/bin/bash
set -e

echo "========================================"
echo "Shimmer Test Runner (Lima VM)"
echo "========================================"

SHIMMER_PATH="$SHIMMER_PATH"

if ! sudo test -b /dev/nullb0; then
    echo "ERROR: /dev/nullb0 not found - VM not provisioned correctly"
    exit 1
fi

echo "Using ZNS device: /dev/nullb0"

echo "Setting up ZoneFS..."

if mountpoint -q /mnt/zonefs; then
    echo "Unmounting existing ZoneFS..."
    sudo umount -f /mnt/zonefs
fi

echo "Resetting zones on /dev/nullb0..."
sudo blkzone reset /dev/nullb0

echo "Formatting ZoneFS..."
sudo mkdir -p /mnt/zonefs
sudo mkzonefs -f /dev/nullb0
sudo mount -t zonefs -o rw,explicit-open /dev/nullb0 /mnt/zonefs

if ! mountpoint -q /mnt/zonefs; then
    echo "ERROR: Failed to mount ZoneFS"
    exit 1
fi

echo "ZoneFS mounted at /mnt/zonefs"

echo "Setting permissions on zone files..."
sudo chmod -R 666 /mnt/zonefs/seq/ 2>/dev/null || echo "Could not change zone file permissions"

echo "Cleaning shimmer metadata directory..."
sudo rm -rf /mnt/shimmer
sudo mkdir -p /mnt/shimmer
sudo chmod 777 /mnt/shimmer
echo "Shimmer metadata directory: /mnt/shimmer"

if [ ! -f "$SHIMMER_PATH/target/release/libshimmer.so" ]; then
    echo "ERROR: Shimmer library not found at $SHIMMER_PATH/target/release/libshimmer.so"
    exit 1
fi

echo ""
echo "Running RocksDB benchmark with Shimmer..."
echo "Test directory: /mnt/shimmer"
echo "Shimmer library: $SHIMMER_PATH/target/release/libshimmer.so"
echo "ZoneFS mount: /mnt/zonefs"
echo ""

echo "Zone files before test:"
ls -lh /mnt/zonefs/seq/ 2>/dev/null || echo "No zone files yet"
echo ""
echo "Zone file permissions:"
ls -la /mnt/zonefs/seq/ 2>/dev/null || echo "Cannot list zone files"
echo ""
echo "ZoneFS mount info:"
mount | grep zonefs
echo ""

# Run db_bench with Shimmer preloaded using FIFO compaction
# Parameters scaled down for emulated environment (128 MiB zones, 16 zones)
echo "Running db_bench..."
sudo LD_PRELOAD="$SHIMMER_PATH/target/release/libshimmer.so" db_bench \
    --db=/mnt/shimmer \
    --benchmarks="fillseq,fillrandom,readseq,readrandom,overwrite,readwhilewriting,stats" \
    --num=500000 \
    --verify_checksum=false \
    --key_size=20 \
    --value_size=400 \
    --target_file_size_base=33554432 \
    --target_file_size_multiplier=1 \
    --wal_bytes_per_sync=16384 \
    --use_direct_io_for_flush_and_compaction \
    --seed=420 \
    --histogram \
    --compaction_style=2 2>&1 | tee /tmp/db_bench.log

DB_BENCH_EXIT=\${PIPESTATUS[0]}

if grep -q "put error:\|Corruption:\|ERROR:" /tmp/db_bench.log; then
    echo "ERROR: db_bench encountered errors"
    echo "Last 50 lines of output:"
    tail -50 /tmp/db_bench.log
    exit 1
fi

if [ \$DB_BENCH_EXIT -ne 0 ]; then
    echo "ERROR: db_bench failed with exit code \$DB_BENCH_EXIT"
    echo "Last 50 lines of output:"
    tail -50 /tmp/db_bench.log
    exit 1
fi

echo ""
echo "Zone files after test:"
ls -lh /mnt/zonefs/seq/ 2>/dev/null || echo "No zone files"

echo ""
echo "========================================"
echo "Test Complete!"
echo "========================================"
EOF
)

echo ""
echo "Executing tests in VM..."
echo ""
"$SSH_HELPER" "bash -s" <<<"$TEST_SCRIPT"

TEST_EXIT=$?

echo ""
print_header "Test Results"
if [ $TEST_EXIT -eq 0 ]; then
    print_success "Tests PASSED"
    exit 0
else
    print_error "Tests FAILED"
    exit 1
fi
