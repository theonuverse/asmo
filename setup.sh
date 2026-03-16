#!/data/data/com.termux/files/usr/bin/sh
# asmo setup script for Termux
# https://github.com/theonuverse/asmo

set -e

REPO="https://github.com/theonuverse/asmo.git"
BIN="$PREFIX/bin/asmo"
SV_DIR="$PREFIX/etc/sv/asmo"
LOG_DIR="$PREFIX/var/log/asmo"

echo ""
echo "=== asmo v0.5.0 setup ==="
echo ""

# ── 1. Update packages & install prerequisites ───────────────────────────────

echo "[1/7] Updating package index..."
pkg update -y

echo "[2/7] Installing build dependencies..."
pkg install -y git rust

echo "[3/7] Checking for termux-services..."
if ! command -v sv >/dev/null 2>&1; then
    echo ""
    echo "  termux-services is not installed — installing now."
    pkg install -y termux-services
    echo ""
    echo "  IMPORTANT: termux-services requires closing and reopening Termux"
    echo "  to become fully active. Please do that now, then re-run this script."
    echo ""
    exit 0
fi
echo "  found: $(command -v sv)"

# ── 2. Clone and build ───────────────────────────────────────────────────────

echo "[4/7] Cloning asmo..."
cd ~
rm -rf ~/asmo
git clone "$REPO" asmo
cd asmo

echo "[5/7] Building release binary (first build takes a few minutes)..."
cargo build --release

echo "[6/7] Installing binary..."
cp target/release/asmo "$BIN"
chmod +x "$BIN"
echo "  installed: $BIN"

# ── 3. Configure the runit service ──────────────────────────────────────────

echo "[7/7] Configuring asmo service..."
mkdir -p "$SV_DIR/log"
mkdir -p "$LOG_DIR"

# Service run script.
# Direct exec (no shell wrapper) so runit supervises the real process and
# the full process tree is cleanly managed on stop/restart.
# Stderr is merged into stdout so everything flows through the log pipeline.
# Set RUST_LOG to adjust verbosity: info (default), debug (verbose), warn (quiet).
cat > "$SV_DIR/run" << 'RUNEOF'
#!/data/data/com.termux/files/usr/bin/sh
# Merge stderr into stdout so all output reaches the log pipeline.
exec 2>&1
# Uncomment the next line for verbose debug logging:
# export RUST_LOG=debug
exec asmo
RUNEOF

# Log script: svlogd rotates logs with timestamps under $LOG_DIR.
# The -tt flag prefixes each line with a human-readable timestamp.
cat > "$SV_DIR/log/run" << LOGEOF
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt $LOG_DIR
LOGEOF

chmod +x "$SV_DIR/run"
chmod +x "$SV_DIR/log/run"

# Register the service so it starts automatically on every Termux session.
# sv-enable creates the supervised symlink under $PREFIX/var/service/.
sv-enable asmo

# Start the service immediately — no need to wait for runsvdir's next scan.
sv up asmo

# ── 4. Summary and next-step guidance ────────────────────────────────────────

echo ""
echo "=== Setup complete ==="
echo ""
echo "  Current service state:"
sv status asmo || true
echo ""
echo "  Service commands:"
echo "    sv status asmo              show current state (run / down / finish)"
echo "    sv up asmo                  start the service"
echo "    sv down asmo                stop the service"
echo "    sv restart asmo             graceful stop + restart"
echo "    sv-enable asmo              enable auto-start on session open (already done)"
echo "    sv-disable asmo             remove from auto-start"
echo ""
echo "  Viewing logs:"
echo "    tail -f $LOG_DIR/current    follow live output"
echo "    tail -50 $LOG_DIR/current   last 50 lines"
echo ""
echo "  API checks:"
echo "    curl -s localhost:3000/health | jq .     monitor health"
echo "    curl -s localhost:3000/debug  | jq .     deep diagnostics"
echo ""
echo "  To enable verbose debug logging, edit the run script:"
echo "    nano $SV_DIR/run"
echo "    # Uncomment the 'export RUST_LOG=debug' line, then:"
echo "    sv restart asmo"
echo ""
echo "  See README section 'Termux Service Management' for full details."
echo ""