# Valet

[![Lint](https://github.com/shimplify/valet/actions/workflows/lint.yml/badge.svg)](https://github.com/shimplify/valet/actions/workflows/lint.yml)
[![Build](https://github.com/shimplify/valet/actions/workflows/build.yml/badge.svg)](https://github.com/shimplify/valet/actions/workflows/build.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

*Shimmer is the development codename for Valet*

## About

This repository contains the implementation of:

**Devashish R. Purandare, Peter Alvaro, Avani Wildani, Darrell D. E. Long, and Ethan L. Miller. 2025. Valet: Efficient Data Placement on Modern SSDs. In ACM Symposium on Cloud Computing (SoCC '25), November 19–21, 2025, Online, USA. ACM, New York, NY, USA, 14 pages.** https://doi.org/10.1145/3772052.3772256

Valet (`libshimmer.so`) is a dynamic library that intercepts and redirects file operations from append-only applications to ZNS (Zoned Namespace) storage devices formatted as [ZoneFS](https://docs.kernel.org/filesystems/zonefs.html). Using `LD_PRELOAD`, it replaces libc functions to provide transparent integration with ZNS SSDs—enabling easy adoption of novel storage interfaces without application changes.

The `automated-hints` branch includes ML-based stream classification for automated data placement.

## Quick Start

The easiest way to build and test Valet is using the provided Lima VM setup with ZNS emulation:

```bash
# Navigate to vm directory
cd vm

# Start the VM (first run provisions automatically)
./lima-start.sh start

# Build and run tests
./lima-test.sh
```

The VM automatically provisions:
- Ubuntu 24.04 environment
- ZNS device emulation via null_blk (2GB, 128MiB zones)
- ZoneFS formatted and mounted at `/mnt/zonefs`
- RocksDB with db_bench tool
- All build dependencies

## Building

Valet is Linux-only and built as a shared library:

```bash
# Release build (cross-compiles on macOS)
make release

# Debug build
make build

# With optional features
make release FEATURES=io_uring

# Format and lint
make fmt
make clippy

# Generate documentation
make doc
```

On macOS, the Makefile automatically cross-compiles to `aarch64-unknown-linux-gnu`.

## Usage

### With RocksDB

```bash
sudo LD_PRELOAD=./target/release/libshimmer.so db_bench \
    --db=/mnt/shimmer \
    --benchmarks="fillrandom,readseq" \
    --num=5000000 \
    --use_direct_io_for_flush_and_compaction \
    --compaction_style=2
```

The `--compaction_style=2` (FIFO) is recommended for best compatibility with Valet's append-only design.

### With Other Applications

Preload Valet with any append-only application:

```bash
sudo LD_PRELOAD=./target/release/libshimmer.so <your-application>
```

Files written to paths under `/mnt/shimmer` will be redirected to ZNS zones.

## Architecture

### Core Modules

- **replacements.rs**: Libc function replacements for intercepting file operations (open, write, close, unlink, etc.)
- **state.rs**: Data structures and state management for zoned devices
- **persist.rs**: Persistence layer for writing buffers to zones and reading from extents
- **helpers.rs**: Helper utilities for buffer and zone management
- **statics.rs**: Global state management for Shimmer
- **metadata.rs**: Metadata persistence for state recovery
- **learning.rs**: ML-based stream classification (`automated-hints` branch)

### Zone Management

Valet manages ZNS devices through:
- **Automatic zone detection**: Zone count enumerated from `/mnt/zonefs/seq/`, size/capacity read from file metadata
- **Stream-based allocation**: Files mapped to sequential zones, with dedicated zones per stream (SSTable, LOG)
- **Zone rotation**: Automatic zone switching when capacity is reached
- **Garbage collection**: Reclaims zones with 100% deleted extents
- **Buffer management**: Configurable extent sizes with buffer pooling

On startup, Valet detects the zone configuration:
```
Detected 15 zones from /mnt/zonefs/seq/, size=134217728 bytes (128 MiB), capacity=134217728 bytes (128 MiB)
```

## Branches

- **`automated-hints`**: ML-based stream classification and automated data placement
- **`main`**: Base implementation
- **`hinted`**: Hints-only approach (ideal for stream/FDP/filesystem integration)

## Requirements

**For VM Testing:**
- macOS or Linux host
- [Lima](https://lima-vm.io/) VM manager

**For Real Hardware:**
- Linux system with ZoneFS kernel support
- ZNS SSD (e.g., Western Digital ZN540)
- ZoneFS formatted and mounted at `/mnt/zonefs`
- Random-write filesystem for metadata (LOCK files, etc.)
- Root access for direct I/O

## Testing

```bash
# Standard test with FIFO compaction
./vm/lima-test.sh

# Test different compaction modes
./vm/test-compaction-styles.sh
```

## Documentation

- **[Setup.md](Setup.md)**: Detailed setup guide, VM usage, and troubleshooting
- **vm/provision.sh**: Complete VM provisioning script
- **vm/lima.yaml**: VM configuration and mount points
- **Generated docs**: Run `make doc` to view API documentation

## VM Management

```bash
# SSH into the VM
./vm/lima-ssh.sh

# Stop the VM
./vm/lima-start.sh stop

# Delete the VM
./vm/lima-start.sh delete

# Check VM status
limactl list
```

The project is mounted at `/mnt/valet` in the VM with automatic sync.

## Citation

If you use this work in your research, please cite:

```bibtex
@inproceedings{purandare2025valet,
  title={Valet: Efficient Data Placement on Modern SSDs},
  author={Purandare, Devashish R. and Alvaro, Peter and Wildani, Avani and Long, Darrell D. E. and Miller, Ethan L.},
  booktitle={ACM Symposium on Cloud Computing (SoCC '25)},
  year={2025},
  month={November},
  location={Online, USA},
  publisher={ACM},
  address={New York, NY, USA},
  pages={14},
  doi={10.1145/3772052.3772256}
}
```

## License

MIT License - see [LICENSE](LICENSE) file for details.
