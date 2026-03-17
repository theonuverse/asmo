#!/data/data/com.termux/files/usr/bin/sh

set -e

SV_DIR="$PREFIX/etc/sv/asmo"
LOG_SERVICE_DIR="$SV_DIR/log"
SERVICE_LINK="$PREFIX/var/service/asmo"
LOG_DIR="$PREFIX/var/log/asmo"
SERVICE_NAME="asmo"
VERIFY_TIMEOUT=20
VERIFY_INTERVAL=1

print_banner() {
    printf '\n'
    printf '%s\n' '----------------------------------------'
    printf '%s\n' 'Asmo Termux Service Setup'
    printf '%s\n' '----------------------------------------'
}

print_section() {
    printf '\n[%s]\n' "$1"
}

print_detail() {
    printf '  - %s\n' "$1"
}

print_success() {
    printf '  [OK] %s\n' "$1"
}

print_warn() {
    printf '  [WARN] %s\n' "$1"
}

print_error() {
    printf '  [ERROR] %s\n' "$1" >&2
}

print_hint() {
    printf '      %s\n' "$1"
}

require_asmo_binary() {
    if ! command -v asmo >/dev/null 2>&1; then
        print_error "'asmo' is not available in PATH."
        print_error "Install the binary first, then rerun this script."
        exit 1
    fi
}

ensure_termux_services() {
    if command -v sv >/dev/null 2>&1; then
        return
    fi

    print_section "Dependencies"
    print_detail "Installing termux-services."
    pkg install -y termux-services
    print_success "termux-services installed."
    if [ -f "$PREFIX/etc/profile.d/start-services.sh" ]; then
        print_detail "Starting runsvdir for the current shell session."
        . "$PREFIX/etc/profile.d/start-services.sh"
        sleep 1
    fi
}

require_rish() {
    if command -v rish >/dev/null 2>&1; then
        return
    fi

    print_error "'rish' is not available in PATH."
    print_error "Install or expose rish first, then rerun this script."
    exit 1
}

ensure_runsvdir() {
    if pgrep -x runsvdir >/dev/null 2>&1; then
        return
    fi

    if [ -f "$PREFIX/etc/profile.d/start-services.sh" ]; then
        print_detail "Starting runsvdir for the current shell session."
        . "$PREFIX/etc/profile.d/start-services.sh"
        sleep 1
    fi
}

remove_existing_service() {
    if [ -L "$SERVICE_LINK" ] || [ -d "$SV_DIR" ] || [ -d "$LOG_DIR" ]; then
        print_section "Cleanup"
        print_detail "Removing the previous asmo service registration."
        sv down "$SERVICE_NAME" >/dev/null 2>&1 || true
        rm -rf "$SERVICE_LINK"
        rm -rf "$SV_DIR"
        rm -rf "$LOG_DIR"
        print_success "Previous service registration removed."
    fi
}

create_service_files() {
    print_section "Install"
    print_detail "Creating the runit service structure."
    mkdir -p "$LOG_SERVICE_DIR"
    mkdir -p "$LOG_DIR"
    mkdir -p "$PREFIX/var/service"

    cat > "$SV_DIR/run" << 'EOF'
#!/data/data/com.termux/files/usr/bin/sh
exec script -q -c "asmo" /dev/null 2>&1
EOF

    cat > "$LOG_SERVICE_DIR/run" << 'EOF'
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt /data/data/com.termux/files/usr/var/log/asmo
EOF

    chmod +x "$SV_DIR/run"
    chmod +x "$LOG_SERVICE_DIR/run"
    print_success "Service files written."
    print_hint "Service directory: $SV_DIR"
    print_hint "Log directory:     $LOG_DIR"
}

enable_service() {
    print_section "Enable"
    print_detail "Registering the service with runit."
    ln -snf "$SV_DIR" "$SERVICE_LINK"
    ensure_runsvdir
    print_success "Service link created at $SERVICE_LINK."
}

verify_service() {
    print_section "Verify"
    print_detail "Waiting for runit to detect and start the service."

    attempts=0
    max_attempts=$((VERIFY_TIMEOUT / VERIFY_INTERVAL))
    while [ "$attempts" -lt "$max_attempts" ]; do
        if [ -e "$SV_DIR/supervise/ok" ] || [ -e "$SERVICE_LINK/supervise/ok" ]; then
            status="$(sv status "$SERVICE_NAME" 2>&1 || true)"
            case "$status" in
                run:*)
                    print_success "Service is running."
                    print_hint "$status"
                    print_hint "Logs: tail -f $LOG_DIR/current"
                    return
                    ;;
            esac
        fi

        attempts=$((attempts + 1))
        sleep "$VERIFY_INTERVAL"
    done

    print_error "Service did not come up in time."
    print_hint "sv status $SERVICE_NAME"
    print_hint "tail -50 $LOG_DIR/current"
    exit 1
}

print_banner
ensure_termux_services
require_rish
require_asmo_binary
remove_existing_service
create_service_files
enable_service
verify_service

printf '\n'
printf '%s\n' 'Setup complete.'