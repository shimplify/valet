#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VM_NAME="shimmer"

export LIMA_HOME="$SCRIPT_DIR/.lima"
mkdir -p "$LIMA_HOME"

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
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

print_warning() {
    echo -e "${YELLOW}⚠${NC} $1"
}

print_error() {
    echo -e "${RED}✗${NC} $1"
}

if ! command -v limactl &>/dev/null; then
    print_error "Lima is not installed"
    echo ""
    echo "Please install Lima:"
    echo "  macOS: brew install lima"
    echo "  Linux: See https://lima-vm.io/docs/installation/"
    exit 1
fi

COMMAND="${1:-start}"

case "$COMMAND" in
start)
    print_header "Starting Shimmer Test VM"

    if limactl list | grep -q "^$VM_NAME"; then
        VM_STATUS=$(limactl list | grep "^$VM_NAME" | awk '{print $2}')
        if [ "$VM_STATUS" = "Running" ]; then
            print_warning "VM is already running"
            exit 0
        else
            print_warning "VM exists but is not running. Starting..."
            limactl start "$VM_NAME"
        fi
    else
        print_success "Creating new VM from lima.yaml"
        cd "$SCRIPT_DIR"
        limactl start --name="$VM_NAME" --tty=false lima.yaml
    fi

    print_success "VM is running"
    echo ""
    echo "To connect: limactl shell $VM_NAME"
    echo "Or use: ./lima-ssh.sh"
    ;;

stop)
    print_header "Stopping Shimmer Test VM"
    limactl stop "$VM_NAME"
    print_success "VM stopped"
    ;;

delete)
    print_header "Deleting Shimmer Test VM"
    limactl delete "$VM_NAME"
    print_success "VM deleted"
    ;;

restart)
    print_header "Restarting Shimmer Test VM"
    limactl stop "$VM_NAME" || true
    limactl start "$VM_NAME"
    print_success "VM restarted"
    ;;

status)
    print_header "VM Status"
    limactl list | grep -E "(NAME|^$VM_NAME)" || echo "VM not found"
    ;;

shell)
    limactl shell "$VM_NAME"
    ;;

*)
    echo "Usage: $0 {start|stop|delete|restart|status|shell}"
    echo ""
    echo "Commands:"
    echo "  start    - Start or create the VM"
    echo "  stop     - Stop the VM"
    echo "  delete   - Delete the VM"
    echo "  restart  - Restart the VM"
    echo "  status   - Show VM status"
    echo "  shell    - Open a shell in the VM"
    exit 1
    ;;
esac
