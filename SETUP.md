# Valet Setup Guide

This guide provides detailed setup instructions for Valet (Shimmer). For a quick overview, see [README.md](README.md).

## Prerequisites

- macOS or Linux host
- [Lima](https://lima-vm.io/) VM manager
  ```bash
  # On macOS
  brew install lima

  # On Linux
  # See https://lima-vm.io/docs/installation/
  ```

## VM Setup and Testing

```bash
# Navigate to vm directory
cd vm

# Start the VM (first run provisions automatically)
./lima-start.sh start

# Build and run tests
./lima-test.sh

# The test runs RocksDB's db_bench with Shimmer preloaded
# using an emulated ZNS device
```

### Manual Build in VM

```bash
# SSH into VM
./lima-ssh.sh

# Project is mounted at /mnt/valet
cd /mnt/valet

# Build
make release

# The library will be at target/release/libshimmer.so
```

## How the VM Works

The VM setup (see `vm/lima.yaml` and `vm/provision.sh`) performs:

1. **Mounts your local repository** at `/mnt/valet` in the VM (changes sync automatically)
2. **Installs dependencies** (build tools, Rust, RocksDB dependencies)
3. **Creates emulated ZNS device** using null_blk kernel module
   - 2 GB device with 128 MiB zones
   - Device appears as `/dev/nullb0`
4. **Formats and mounts ZoneFS** at `/mnt/zonefs`
5. **Builds RocksDB** from source with db_bench tool

### Zone Configuration

The emulated device provides:
- 16 total zones of 128 MiB each
- 15 usable zones (1 reserved for ZoneFS metadata)
- Zone files accessible at `/mnt/zonefs/seq/0` through `/mnt/zonefs/seq/14`

Valet auto-detects this configuration at startup:
```
Detected 15 zones from /mnt/zonefs/seq/, size=134217728 bytes (128 MiB), capacity=134217728 bytes (128 MiB)
```

## Using with Real ZNS Hardware

While the VM provides an emulated environment, Valet works with real NVMe ZNS SSDs. The requirements are:

1. **Linux system** with ZoneFS kernel support
2. **ZNS SSD** (e.g., Western Digital ZN540)
3. **ZoneFS formatted and mounted** at `/mnt/zonefs`

The setup process is similar to what `vm/provision.sh` does, but uses a real NVMe device instead of null_blk. Consult the provisioning script for the exact steps.

## Troubleshooting

### VM Issues

If the VM fails to start or provision:

```bash
# Check status
limactl list

# View logs
limactl show-log shimmer

# Recreate VM
./lima-start.sh delete
./lima-start.sh start
```

### Zone Detection Fails

If Valet can't detect zones, verify ZoneFS is mounted in the VM:

```bash
./lima-ssh.sh
mount | grep zonefs
ls -la /mnt/zonefs/seq/
```

### Test Failures

Check that the ZNS device is properly configured:

```bash
./lima-ssh.sh
sudo blkzone report /dev/nullb0 | head
```

## Additional Resources

- **vm/provision.sh**: Complete provisioning script showing exact setup steps
- **vm/build-rocksdb.sh**: RocksDB build configuration
- **vm/lima.yaml**: VM configuration and mount points
- **[README.md](README.md)**: Project overview and quick start
- **[CLAUDE.md](CLAUDE.md)**: Development guidelines and architecture notes
