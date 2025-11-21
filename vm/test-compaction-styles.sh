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
YELLOW='\033[1;33m'
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

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_header "Testing Shimmer with Different Compaction Styles"

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

# Compaction styles to test
# 0 = Level-based (default)
# 1 = Universal
# 2 = FIFO (currently used in tests)
COMPACTION_STYLES=(2 0 1)
COMPACTION_NAMES=("FIFO" "Level-based" "Universal")

# Store results (will use indexed array instead of associative)
RESULTS_0=""
RESULTS_1=""
RESULTS_2=""

for i in "${!COMPACTION_STYLES[@]}"; do
    STYLE="${COMPACTION_STYLES[$i]}"
    NAME="${COMPACTION_NAMES[$i]}"

    print_header "Testing Compaction Style: $NAME (style=$STYLE)"

    TEST_SCRIPT=$(
        cat <<EOF
#!/bin/bash
set -e

echo "========================================"
echo "Shimmer Test: $NAME Compaction (style=$STYLE)"
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

if [ ! -f "\$SHIMMER_PATH/target/release/libshimmer.so" ]; then
    echo "ERROR: Shimmer library not found at \$SHIMMER_PATH/target/release/libshimmer.so"
    exit 1
fi

echo ""
echo "Running RocksDB benchmark with Shimmer..."
echo "Test directory: /mnt/shimmer"
echo "Shimmer library: \$SHIMMER_PATH/target/release/libshimmer.so"
echo "ZoneFS mount: /mnt/zonefs"
echo "Compaction style: $NAME ($STYLE)"
echo ""

echo "Zone files before test:"
ls -lh /mnt/zonefs/seq/ 2>/dev/null || echo "No zone files yet"
echo ""

# Run db_bench with Shimmer preloaded
# Parameters scaled down for emulated environment (128 MiB zones, 16 zones)
echo "Running db_bench with compaction_style=$STYLE..."
sudo LD_PRELOAD="\$SHIMMER_PATH/target/release/libshimmer.so" db_bench \
    --db=/mnt/shimmer \
    --benchmarks="fillseq,readseq" \
    --num=100000 \
    --verify_checksum=false \
    --key_size=20 \
    --value_size=400 \
    --target_file_size_base=33554432 \
    --target_file_size_multiplier=1 \
    --wal_bytes_per_sync=16384 \
    --use_direct_io_for_flush_and_compaction \
    --seed=420 \
    --histogram \
    --compaction_style=$STYLE 2>&1 | tee /tmp/db_bench_style_${STYLE}.log

DB_BENCH_EXIT=\${PIPESTATUS[0]}

if grep -qi "put error:\|Corruption:\|Assertion.*failed\|IO error:\|fatal" /tmp/db_bench_style_${STYLE}.log; then
    echo "ERROR: db_bench encountered errors"
    echo "Last 100 lines of output:"
    tail -100 /tmp/db_bench_style_${STYLE}.log
    exit 1
fi

if [ \$DB_BENCH_EXIT -ne 0 ]; then
    echo "ERROR: db_bench failed with exit code \$DB_BENCH_EXIT"
    echo "Last 100 lines of output:"
    tail -100 /tmp/db_bench_style_${STYLE}.log
    exit 1
fi

echo ""
echo "Zone files after test:"
ls -lh /mnt/zonefs/seq/ 2>/dev/null || echo "No zone files"

echo ""
echo "========================================"
echo "Test Complete for $NAME!"
echo "========================================"
EOF
    )

    echo ""
    echo "Executing test for $NAME compaction..."
    echo ""

    if "$SSH_HELPER" "bash -s" <<<"$TEST_SCRIPT"; then
        eval "RESULTS_${STYLE}=PASSED"
        print_success "$NAME compaction (style=$STYLE): PASSED"
    else
        eval "RESULTS_${STYLE}=FAILED"
        print_error "$NAME compaction (style=$STYLE): FAILED"

        # Retrieve the log for analysis
        echo ""
        print_warning "Retrieving failure logs for analysis..."
        "$SSH_HELPER" "cat /tmp/db_bench_style_${STYLE}.log 2>/dev/null | tail -100" || echo "Could not retrieve logs"
    fi

    echo ""
    echo "---"
    sleep 2
done

print_header "Final Results Summary"
echo ""
for i in "${!COMPACTION_STYLES[@]}"; do
    STYLE="${COMPACTION_STYLES[$i]}"
    NAME="${COMPACTION_NAMES[$i]}"
    eval "RESULT=\$RESULTS_${STYLE}"

    if [ "$RESULT" == "PASSED" ]; then
        print_success "$NAME (style=$STYLE): $RESULT"
    else
        print_error "$NAME (style=$STYLE): $RESULT"
    fi
done

echo ""
print_header "Analysis"
echo ""
echo "Checking for common patterns in failures..."
echo ""

# Check if only FIFO passed
if [ "$RESULTS_2" == "PASSED" ] && [ "$RESULTS_0" == "FAILED" ] && [ "$RESULTS_1" == "FAILED" ]; then
    print_warning "Only FIFO compaction passed!"
    echo ""
    echo "Possible reasons why Level-based and Universal compaction fail:"
    echo "  1. These compaction styles perform in-place updates/rewrites"
    echo "  2. Shimmer is designed for append-only workloads (as noted in CLAUDE.md)"
    echo "  3. FIFO compaction doesn't rewrite files, only drops old data"
    echo "  4. Level/Universal try to modify existing SST files during compaction"
    echo ""
    echo "Shimmer's design constraints may be incompatible with non-FIFO compaction."
fi

echo ""
print_header "Done"
