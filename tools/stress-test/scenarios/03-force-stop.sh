#!/usr/bin/env bash

# Android app force-stop cycle
ITERATIONS=50
SUCCESS=0
FAIL=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPORTS_DIR="${SCRIPT_DIR}/../reports"
SUMMARY_FILE="${REPORTS_DIR}/summary.md"
L1_DIR="${SCRIPT_DIR}/../../../l1/daemon"
mkdir -p "${REPORTS_DIR}"

echo "🔨 Starting daemon in background for Android force-stop test..."
cd "${L1_DIR}"
cargo run --release > "${REPORTS_DIR}/vortex-force-stop.log" 2>&1 &
DAEMON_PID=$!

# Wait for initial connection
sleep 5

for i in $(seq 1 $ITERATIONS); do
    echo "[$i/$ITERATIONS] Force stopping and restarting Android app..."
    
    # Force stop
    adb shell am force-stop com.vortex.lab.a1
    sleep 2
    
    # Clear log
    > "${REPORTS_DIR}/vortex-force-stop.log"
    
    # Restart foreground service
    adb shell am start-foreground-service -n com.vortex.lab.a1/.service.VortexService > /dev/null 2>&1
    
    # Wait for connection indicator
    CONNECTED=false
    for _ in {1..200}; do # 20s timeout
        if grep -q -iE "trusted_connected|paired|session established|connected to" "${REPORTS_DIR}/vortex-force-stop.log" 2>/dev/null; then
            CONNECTED=true
            break
        fi
        sleep 0.1
    done
    
    if [ "$CONNECTED" = true ]; then
        SUCCESS=$((SUCCESS + 1))
        echo "  ✅ Reconnected after Android app restart in <20s"
    else
        FAIL=$((FAIL + 1))
        echo "  ❌ Timeout after Android app restart"
        cp "${REPORTS_DIR}/vortex-force-stop.log" "${REPORTS_DIR}/failed-03-$i.log"
    fi
    
    sleep 2
done

# Cleanup
kill -TERM $DAEMON_PID 2>/dev/null
wait $DAEMON_PID 2>/dev/null || true

RATE=$(( (SUCCESS * 100) / ITERATIONS ))
echo "Results: $SUCCESS/$ITERATIONS passed, $FAIL failed (${RATE}%)"
echo "| Force stop | $SUCCESS/$ITERATIONS | $FAIL | ${RATE}% |" >> "${SUMMARY_FILE}"
