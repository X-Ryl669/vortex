#!/usr/bin/env bash

# 100 reconnect cycle
ITERATIONS=100
SUCCESS=0
FAIL=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPORTS_DIR="${SCRIPT_DIR}/../reports"
SUMMARY_FILE="${REPORTS_DIR}/summary.md"
L1_DIR="${SCRIPT_DIR}/../../../l1/daemon"
mkdir -p "${REPORTS_DIR}"

# Build daemon in release mode before test to avoid timeouts during compilation
echo "🔨 Building daemon..."
cd "${L1_DIR}"
cargo build --release

for i in $(seq 1 $ITERATIONS); do
    echo "[$i/$ITERATIONS] Starting reconnect test..."
    
    # Start daemon
    cd "${L1_DIR}"
    cargo run --release > "${REPORTS_DIR}/vortex-test-$i.log" 2>&1 &
    DAEMON_PID=$!
    
    # Wait for connection indicator
    CONNECTED=false
    for _ in {1..400}; do # 40s timeout
        if grep -q -iE "trusted_connected|paired|session established|connected to" "${REPORTS_DIR}/vortex-test-$i.log" 2>/dev/null; then
            CONNECTED=true
            break
        fi
        sleep 0.1
    done
    
    if [ "$CONNECTED" = true ]; then
        SUCCESS=$((SUCCESS + 1))
        echo "  ✅ Connected in <40s"
        rm "${REPORTS_DIR}/vortex-test-$i.log" # Cleanup on success
    else
        FAIL=$((FAIL + 1))
        echo "  ❌ Timeout"
        mv "${REPORTS_DIR}/vortex-test-$i.log" "${REPORTS_DIR}/failed-01-$i.log"
    fi
    
    # Cleanup
    kill -TERM $DAEMON_PID 2>/dev/null
    wait $DAEMON_PID 2>/dev/null || true
    sleep 2
done

RATE=$(( (SUCCESS * 100) / ITERATIONS ))
echo "Results: $SUCCESS/$ITERATIONS passed, $FAIL failed (${RATE}%)"
echo "| Reconnect loop | $SUCCESS/$ITERATIONS | $FAIL | ${RATE}% |" >> "${SUMMARY_FILE}"
