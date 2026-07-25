#!/usr/bin/env bash

# BT on/off cycle (Linux)
ITERATIONS=50
SUCCESS=0
FAIL=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPORTS_DIR="${SCRIPT_DIR}/../reports"
SUMMARY_FILE="${REPORTS_DIR}/summary.md"
L1_DIR="${SCRIPT_DIR}/../../../l1/daemon"
mkdir -p "${REPORTS_DIR}"

echo "🔨 Starting daemon in background for BT toggle test..."
cd "${L1_DIR}"
cargo run --release > "${REPORTS_DIR}/vortex-bt-toggle.log" 2>&1 &
DAEMON_PID=$!

# Wait for initial connection
sleep 5

for i in $(seq 1 $ITERATIONS); do
    echo "[$i/$ITERATIONS] Toggling Bluetooth (Off -> On)..."
    
    # Toggle BT off
    bluetoothctl power off > /dev/null
    sleep 2
    
    # Clear log to check for new connection
    > "${REPORTS_DIR}/vortex-bt-toggle.log"
    
    # Toggle BT on
    bluetoothctl power on > /dev/null
    
    # Wait for connection indicator
    CONNECTED=false
    for _ in {1..200}; do # 20s timeout
        if grep -q -iE "trusted_connected|paired|session established|connected to" "${REPORTS_DIR}/vortex-bt-toggle.log" 2>/dev/null; then
            CONNECTED=true
            break
        fi
        sleep 0.1
    done
    
    if [ "$CONNECTED" = true ]; then
        SUCCESS=$((SUCCESS + 1))
        echo "  ✅ Reconnected after BT toggle in <20s"
    else
        FAIL=$((FAIL + 1))
        echo "  ❌ Timeout after BT toggle"
        cp "${REPORTS_DIR}/vortex-bt-toggle.log" "${REPORTS_DIR}/failed-02-$i.log"
    fi
    
    sleep 2
done

# Cleanup
kill -TERM $DAEMON_PID 2>/dev/null
wait $DAEMON_PID 2>/dev/null || true
bluetoothctl power on > /dev/null # ensure it's left on for next tests

RATE=$(( (SUCCESS * 100) / ITERATIONS ))
echo "Results: $SUCCESS/$ITERATIONS passed, $FAIL failed (${RATE}%)"
echo "| BT toggle | $SUCCESS/$ITERATIONS | $FAIL | ${RATE}% |" >> "${SUMMARY_FILE}"
