#!/bin/bash
set -e

UNAME_S=$(uname -s)

echo "========================================"
echo "Installing Shimmer Dependencies"
echo "========================================"
echo "Detected OS: $UNAME_S"
echo ""

if [ "$UNAME_S" = "Darwin" ]; then
    echo "Installing macOS dependencies..."

    if ! command -v brew &>/dev/null; then
        echo "Error: Homebrew is not installed"
        echo "Install from: https://brew.sh"
        exit 1
    fi

    # Lima
    echo "Installing Lima..."
    brew install lima

    # Cross-compilation toolchains
    echo "Installing ARM64 Linux cross-compiler..."
    brew install aarch64-linux-gnu

    echo "Installing x86_64 Linux cross-compiler..."
    brew install x86_64-linux-gnu || echo "x86_64 cross-compiler not available, skipping"

    echo ""
    echo "macOS dependencies installed successfully!"
    echo ""
    echo "Next steps:"
    echo "  1. Start the Lima VM: ./vm/lima-start.sh start"
    echo "  2. Run tests: ./vm/lima-test.sh"

elif [ "$UNAME_S" = "Linux" ]; then
    echo "Installing Linux dependencies..."

    if [ -f /etc/os-release ]; then
        # shellcheck source=/dev/null
        . /etc/os-release
        DISTRO=$ID
    else
        echo "Cannot detect Linux distribution"
        exit 1
    fi

    # Lima
    echo "Installing Lima..."
    if ! command -v limactl &>/dev/null; then
        LIMA_VERSION="0.23.2"
        curl -fsSL "https://github.com/lima-vm/lima/releases/download/v${LIMA_VERSION}/lima-${LIMA_VERSION}-Linux-x86_64.tar.gz" | sudo tar Cxzf /usr/local -
        echo "Lima ${LIMA_VERSION} installed successfully"
    else
        echo "Lima already installed: $(limactl --version)"
    fi

    ARCH=$(uname -m)
    case $DISTRO in
    ubuntu | debian)
        echo "Detected Debian/Ubuntu"
        sudo apt-get update

        # Build tools
        echo "Installing build essentials..."
        sudo apt-get install -y build-essential pkg-config

        # QEMU
        echo "Installing QEMU..."
        if [ "$ARCH" = "x86_64" ]; then
            sudo apt-get install -y qemu-system-x86 qemu-utils
        elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
            sudo apt-get install -y qemu-system-aarch64 qemu-utils
        else
            echo "Error: Unsupported architecture $ARCH"
            exit 1
        fi

        # ZNS tools
        echo "Installing ZNS tools..."
        sudo apt-get install -y nvme-cli "linux-modules-extra-$(uname -r)" zonefs-tools

        # RocksDB dependencies
        echo "Installing RocksDB dependencies..."
        sudo apt-get install -y \
            cmake git wget curl \
            libgflags-dev libsnappy-dev zlib1g-dev \
            libbz2-dev liblz4-dev libzstd-dev

        echo ""
        echo "Linux dependencies installed successfully!"
        ;;

    fedora | rhel | centos)
        echo "Detected Fedora/RHEL/CentOS"

        # Build tools
        echo "Installing build essentials..."
        sudo dnf install -y gcc gcc-c++ make pkg-config

        # QEMU
        echo "Installing QEMU..."
        if [ "$ARCH" = "x86_64" ]; then
            sudo dnf install -y qemu-system-x86 qemu-img
        elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
            sudo dnf install -y qemu-system-aarch64 qemu-img
        else
            echo "Error: Unsupported architecture $ARCH"
            exit 1
        fi

        # ZNS tools
        echo "Installing ZNS tools..."
        sudo dnf install -y nvme-cli kernel-modules-extra

        # RocksDB dependencies
        echo "Installing RocksDB dependencies..."
        sudo dnf install -y \
            cmake git wget curl \
            gflags-devel snappy-devel zlib-devel \
            bzip2-devel lz4-devel libzstd-devel

        echo ""
        echo "Linux dependencies installed successfully!"
        ;;

    arch)
        echo "Detected Arch Linux"

        # Build tools
        echo "Installing build essentials..."
        sudo pacman -S --needed base-devel

        # QEMU
        echo "Installing QEMU..."
        if [ "$ARCH" = "x86_64" ]; then
            sudo pacman -S --needed qemu-system-x86
        elif [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
            sudo pacman -S --needed qemu-system-aarch64
        else
            echo "Error: Unsupported architecture $ARCH"
            exit 1
        fi

        # ZNS tools
        echo "Installing ZNS tools..."
        sudo pacman -S --needed nvme-cli

        # RocksDB dependencies
        echo "Installing RocksDB dependencies..."
        sudo pacman -S --needed \
            cmake git wget curl \
            gflags snappy zlib bzip2 lz4 zstd

        echo ""
        echo "Linux dependencies installed successfully!"
        ;;

    *)
        echo "Unsupported Linux distribution: $DISTRO"
        echo "Please install dependencies manually:"
        echo "  - Lima, QEMU"
        echo "  - Build tools (gcc, make, pkg-config)"
        echo "  - ZNS tools (nvme-cli, zonefs-tools)"
        echo "  - RocksDB dependencies (cmake, gflags, snappy, zlib, bz2, lz4, zstd)"
        exit 1
        ;;
    esac

    echo ""
    echo "Next steps:"
    echo "  1. Start the Lima VM: ./vm/lima-start.sh start"
    echo "  2. Run tests: ./vm/lima-test.sh"

else
    echo "Unsupported operating system: $UNAME_S"
    exit 1
fi

echo ""
echo "========================================"
echo "Installation Complete!"
echo "========================================"
