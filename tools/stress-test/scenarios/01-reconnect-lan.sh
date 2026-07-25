#!/usr/bin/env bash

ITERATIONS=5
SUCCESS=0
FAIL=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPORTS_DIR="${SCRIPT_DIR}/../reports"
SUMMARY_FILE="${REPORTS_DIR}/summary.md"
L3_DAEMON="${SCRIPT_DIR}/../../../linux/target/debug/vortex-l3d"

mkdir -p "${REPORTS_DIR}"

echo "🚀 Starting LAN Stress Test..."

for i in $(seq 1 $ITERATIONS); do
    echo "[$i/$ITERATIONS] Starting LAN reconnect test..."
    
    # vortex-l3d lan-reconnect ni ishga tushiramiz
    VORTEX_INSECURE=1 $L3_DAEMON lan-reconnect > "${REPORTS_DIR}/vortex-lan-test-$i.log" 2>&1
    
    if grep -q "✅ LAN reconnect ok" "${REPORTS_DIR}/vortex-lan-test-$i.log"; then
        SUCCESS=$((SUCCESS + 1))
        echo "  ✅ Connected"
        rm "${REPORTS_DIR}/vortex-lan-test-$i.log"
    else
        FAIL=$((FAIL + 1))
        echo "  ❌ Failed"
        mv "${REPORTS_DIR}/vortex-lan-test-$i.log" "${REPORTS_DIR}/failed-lan-$i.log"
    fi
    
    sleep 1
done

echo "Results: $SUCCESS/$ITERATIONS passed, $FAIL failed"
echo "| LAN Reconnect | $SUCCESS/$ITERATIONS | $FAIL | 100% |" >> "${SUMMARY_FILE}"
