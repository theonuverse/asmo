#!/data/data/com.termux/files/usr/bin/sh
# asmo service setup script for Termux
# Run this from the project directory AFTER:
# 1) cargo build --release
# 2) cp target/release/asmo $PREFIX/bin/asmo

set -e

BIN="$PREFIX/bin/asmo"
SV_DIR="$PREFIX/etc/sv/asmo"
LOG_DIR="$PREFIX/var/log/asmo"
SERVICE_LINK="$PREFIX/var/service/asmo"

echo ""
echo "=== asmo v0.5.0 service setup ==="
echo ""

echo "[1/5] Validating current directory and build artifacts..."
if [ ! -f "Cargo.toml" ] || [ ! -d "src" ]; then
    echo "ERROR: run setup.sh from the asmo project root."
    echo "Current directory: $PWD"
    exit 1
fi

if [ ! -x "$BIN" ]; then
    echo "ERROR: $BIN not found or not executable."
    echo "Build and install first:"
    echo "  cargo build --release"
    echo "  cp target/release/asmo \$PREFIX/bin/asmo"
    exit 1
fi

echo "[2/5] Checking termux-services tooling..."
if ! command -v sv >/dev/null 2>&1; then
    echo "termux-services not found. Installing..."
    pkg install -y termux-services
    echo ""
    echo "Close and reopen Termux once, then re-run setup.sh."
    exit 0
fi
if ! command -v sv-enable >/dev/null 2>&1; then
    echo "ERROR: sv-enable not available. Ensure termux-services is fully initialized."
    echo "Try reopening Termux and running again."
    exit 1
fi
echo "  found: $(command -v sv)"
echo "  found: $(command -v sv-enable)"

# Ensure runsvdir is available in this session. On some devices/services
# setups, this is not active yet even though termux-services is installed.
if ! pgrep -x runsvdir >/dev/null 2>&1; then
    if [ -f "$PREFIX/etc/profile.d/start-services.sh" ]; then
        echo "  starting runsvdir for this session..."
        . "$PREFIX/etc/profile.d/start-services.sh" || true
        sleep 1
    fi
fi

echo "[3/5] Writing service files..."
mkdir -p "$SV_DIR/log"
mkdir -p "$LOG_DIR"
mkdir -p "$PREFIX/var/service"

# rish expects a tty in practice on many devices.
# Keep execution wrapped in `script -q -c` to allocate a pseudo-tty.
cat > "$SV_DIR/run" << 'RUNEOF'
#!/data/data/com.termux/files/usr/bin/sh
exec script -q -c "asmo" /dev/null 2>&1
RUNEOF

cat > "$SV_DIR/log/run" << LOGEOF
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt $LOG_DIR
LOGEOF

chmod +x "$SV_DIR/run"
chmod +x "$SV_DIR/log/run"

echo "[4/5] Enabling service with sv-enable..."
# On some Termux setups sv-enable prints a transient sv error even though
# the enable link is created correctly. Continue and validate via symlink.
if ! sv-enable asmo; then
    echo "  warning: sv-enable returned non-zero; checking service link..."
fi

if [ ! -e "$SERVICE_LINK" ]; then
    echo "  sv-enable did not create link, applying fallback:"
    echo "    ln -snf $SV_DIR $SERVICE_LINK"
    ln -snf "$SV_DIR" "$SERVICE_LINK"
fi

if [ ! -e "$SERVICE_LINK" ]; then
    echo "ERROR: service link is still missing: $SERVICE_LINK"
    echo "Debug checks:"
    echo "  ls -ld $PREFIX/etc/sv/asmo"
    echo "  ls -ld $PREFIX/var/service"
    echo "  ls -l  $PREFIX/var/service"
    exit 1
fi

echo "[5/5] Starting service with sv up..."
sv up asmo || {
    echo "  first sv up failed, retrying once in 1s..."
    sleep 1
    sv up asmo
} || {
    echo ""
    echo "ERROR: sv up asmo failed. Debug steps:"
    echo "  sv status asmo"
    echo "  ls -l $PREFIX/var/service"
    echo "  tail -50 $LOG_DIR/current"
    exit 1
}

echo ""
echo "=== Service setup complete ==="
echo ""
echo "  Current state:"
sv status asmo || true
echo ""
echo "  Service commands:"
echo "    sv status asmo"
echo "    sv up asmo"
echo "    sv down asmo"
echo "    sv restart asmo"
echo "    sv-enable asmo"
echo "    sv-disable asmo"
echo ""
echo "  Logs:"
echo "    tail -f $LOG_DIR/current"
echo ""
echo "  API debug:"
echo "    curl -s localhost:3000/health | jq ."
echo "    curl -s localhost:3000/debug  | jq ."
echo ""