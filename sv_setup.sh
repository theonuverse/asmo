#!/data/data/com.termux/files/usr/bin/sh
# asmo service setup script for Termux
# IMPORTANT: rish (Shizuku) must be available first.
# asmo will not run without rish.

set -e

BIN="$PREFIX/bin/asmo"
SV_DIR="$PREFIX/etc/sv/asmo"
LOG_DIR="$PREFIX/var/log/asmo"
SERVICE_LINK="$PREFIX/var/service/asmo"
MAX_REGISTER_WAIT=20
MAX_START_RETRIES=25

echo ""
echo "=== asmo v0.5.0 service setup ==="
echo ""

echo "[1/6] Validating current directory and build artifacts..."
if [ ! -f "Cargo.toml" ] || [ ! -d "src" ]; then
    echo "ERROR: run sv_setup.sh from the asmo project root."
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

echo "[2/6] Checking termux-services tooling..."
if ! command -v sv >/dev/null 2>&1; then
    echo "termux-services not found. Installing..."
    pkg install -y termux-services
    echo ""
    echo "Close and reopen Termux once, then re-run sv_setup.sh."
    exit 0
fi
echo "  found: $(command -v sv)"
if command -v sv-enable >/dev/null 2>&1; then
    echo "  found: $(command -v sv-enable)"
else
    echo "  sv-enable not found; using symlink activation only"
fi

echo "[3/6] Checking rish availability (required)..."
if ! command -v rish >/dev/null 2>&1; then
    echo "ERROR: rish command not found."
    echo "asmo requires Shizuku/rish and will NOT run without it."
    echo "Install/enable Shizuku and ensure rish is available, then run sv_setup.sh again."
    exit 1
fi

if ! rish -c 'echo rish_ok' >/dev/null 2>&1; then
    echo "ERROR: rish is present but not usable in this session."
    echo "asmo will NOT start without a working rish session."
    echo "Open Shizuku, authorize Termux, verify: rish -c 'echo ok'"
    echo "Then run sv_setup.sh again."
    exit 1
fi
echo "  rish is available and working"

# Ensure runsvdir is available in this session. On some devices/services
# setups, this is not active yet even though termux-services is installed.
if ! pgrep -x runsvdir >/dev/null 2>&1; then
    if [ -f "$PREFIX/etc/profile.d/start-services.sh" ]; then
        echo "  starting runsvdir for this session..."
        . "$PREFIX/etc/profile.d/start-services.sh" || true
        sleep 2
    fi
fi

echo "[4/6] Writing service files..."
# Clean old registration first (important when script is rerun many times).
sv down asmo >/dev/null 2>&1 || true
if command -v sv-disable >/dev/null 2>&1; then
    sv-disable asmo >/dev/null 2>&1 || true
fi
rm -f "$SERVICE_LINK"
sleep 1

# Keep the service directory but remove stale supervise state.
mkdir -p "$SV_DIR/log"
mkdir -p "$LOG_DIR"
mkdir -p "$PREFIX/var/service"
rm -rf "$SV_DIR/supervise" "$SV_DIR/log/supervise"

# rish expects a tty in practice on many devices.
# Keep execution wrapped in `script -q -c` to allocate a pseudo-tty.
cat > "$SV_DIR/run" << 'RUNEOF'
#!/data/data/com.termux/files/usr/bin/sh
exec 2>&1
exec script -q -c "/data/data/com.termux/files/usr/bin/asmo" /dev/null
RUNEOF

cat > "$SV_DIR/log/run" << LOGEOF
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt $LOG_DIR
LOGEOF

chmod +x "$SV_DIR/run"
chmod +x "$SV_DIR/log/run"

echo "[5/6] Enabling service..."
# Primary activation path: direct link in var/service (most reliable in Termux).
ln -snf "$SV_DIR" "$SERVICE_LINK"

# Best-effort call to sv-enable for compatibility. Ignore failures because some
# Termux builds print an sv error here even though the link method works.
if command -v sv-enable >/dev/null 2>&1; then
    sv-enable asmo >/dev/null 2>&1 || true
fi

if [ ! -e "$SERVICE_LINK" ]; then
    echo "ERROR: service link is still missing: $SERVICE_LINK"
    echo "Debug checks:"
    echo "  ls -ld $PREFIX/etc/sv/asmo"
    echo "  ls -ld $PREFIX/var/service"
    echo "  ls -l  $PREFIX/var/service"
    exit 1
fi

echo "  waiting for runit to register service (supervise/ok)..."
count=0
while [ ! -S "$SV_DIR/supervise/ok" ] && [ $count -lt $MAX_REGISTER_WAIT ]; do
    sleep 1
    count=$((count + 1))
done

if [ ! -S "$SV_DIR/supervise/ok" ]; then
    echo "  warning: supervise/ok not ready after ${MAX_REGISTER_WAIT}s; trying sv up retries"
fi

count=0
while [ ! -S "$SV_DIR/log/supervise/ok" ] && [ $count -lt $MAX_REGISTER_WAIT ]; do
    sleep 1
    count=$((count + 1))
done

if [ ! -S "$SV_DIR/log/supervise/ok" ]; then
    echo "  warning: log supervise/ok not ready after ${MAX_REGISTER_WAIT}s; continuing"
fi

echo "[6/6] Starting service with sv up..."
# Reset then start for consistent behavior across reruns.
sv down asmo >/dev/null 2>&1 || true
sleep 1
started=0
retry=0
while [ $retry -lt $MAX_START_RETRIES ]; do
    if sv up asmo >/dev/null 2>&1; then
        started=1
        break
    fi
    retry=$((retry + 1))
    sleep 1
done

if [ $started -ne 1 ]; then
    echo ""
    echo "ERROR: sv up asmo failed after ${MAX_START_RETRIES} retries."
    echo "Debug steps:"
    echo "  sv status asmo"
    echo "  ps -ef | grep runsvdir"
    echo "  ps -ef | grep runsv"
    echo "  ls -l $PREFIX/var/service"
    echo "  ls -la $SV_DIR"
    echo "  tail -50 $LOG_DIR/current"
    exit 1
fi

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