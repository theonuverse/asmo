#!/data/data/com.termux/files/usr/bin/sh
# slim asmo service setup for Termux + runit

set -e

SV_DIR="$PREFIX/etc/sv/asmo"
SERVICE_LINK="$PREFIX/var/service/asmo"
LOG_DIR="$PREFIX/var/log/asmo"
BIN="$PREFIX/bin/asmo"

echo "--- Slim Asmo Setup ---"

if [ ! -f "Cargo.toml" ] || [ ! -d "src" ]; then
    echo "ERROR: run sv_setup.sh from the asmo project root."
    exit 1
fi

if [ ! -x "$BIN" ]; then
    echo "ERROR: $BIN not found. Install binary first:"
    echo "  cp target/release/asmo \$PREFIX/bin/asmo"
    exit 1
fi

if ! command -v sv >/dev/null 2>&1; then
    echo "termux-services missing, installing..."
    pkg install -y termux-services
    echo "Close and reopen Termux, then run sv_setup.sh again."
    exit 0
fi

if ! rish -c 'true' >/dev/null 2>&1; then
    echo "ERROR: rish/Shizuku not active."
    echo "asmo will not start without rish."
    exit 1
fi

# Ensure runsvdir exists for this shell session when possible.
if ! pgrep -x runsvdir >/dev/null 2>&1; then
    if [ -f "$PREFIX/etc/profile.d/start-services.sh" ]; then
        . "$PREFIX/etc/profile.d/start-services.sh" || true
        sleep 1
    fi
fi

# Clean stale registration/state.
sv down asmo >/dev/null 2>&1 || true
rm -f "$SERVICE_LINK"
rm -rf "$SV_DIR"
mkdir -p "$SV_DIR/log"
mkdir -p "$LOG_DIR"
mkdir -p "$PREFIX/var/service"

cat > "$SV_DIR/run" << EOF
#!/data/data/com.termux/files/usr/bin/sh
exec 2>&1
exec script -q -c "$BIN" /dev/null
EOF

cat > "$SV_DIR/log/run" << EOF
#!/data/data/com.termux/files/usr/bin/sh
exec svlogd -tt $LOG_DIR
EOF

chmod +x "$SV_DIR/run" "$SV_DIR/log/run"

# Single activation mechanism: symlink only.
ln -snf "$SV_DIR" "$SERVICE_LINK"

echo "waiting for runit..."
i=0
while [ $i -lt 10 ]; do
    if [ -S "$SV_DIR/supervise/ok" ]; then
        sv up asmo >/dev/null 2>&1 || true
        status="$(sv status asmo 2>&1 || true)"
        case "$status" in
            run:*)
                echo "Service setup complete."
                echo "Status:"
                sv status asmo || true
                echo ""
                echo "Logs: tail -f $LOG_DIR/current"
                exit 0
                ;;
        esac
    fi
    i=$((i + 1))
    sleep 1
done

echo "ERROR: runit did not register/start asmo in time."
echo "Debug checks:"
echo "  sv status asmo"
echo "  ps -ef | grep runsvdir"
echo "  ps -ef | grep runsv"
echo "  ls -l $PREFIX/var/service"
echo "  ls -la $SV_DIR"
echo "  tail -50 $LOG_DIR/current"
exit 1