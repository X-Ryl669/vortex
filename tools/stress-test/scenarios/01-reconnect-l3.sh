#!/usr/bin/env bash

ITERATIONS=5
SUCCESS=0
FAIL=0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPORTS_DIR="${SCRIPT_DIR}/../reports"
SUMMARY_FILE="${REPORTS_DIR}/summary.md"
L3_DAEMON="${SCRIPT_DIR}/../../../linux/target/debug/vortex-l3d"
PEER_ADDR="c6bbcc880502" # ADB orqali ulangan telefon (BD_ADDR emas, lekin bluer uni topishi mumkin yoki biz aniq manzilni bilishimiz kerak)

mkdir -p "${REPORTS_DIR}"

# Telefonning Bluetooth manzili (BD_ADDR) kerak. Uni ADB orqali olamiz.
BD_ADDR=$(adb shell settings get secure bluetooth_address)
if [ -z "$BD_ADDR" ] || [ "$BD_ADDR" = "null" ]; then
    # Agar manzilni olib bo'lmasa, loglardan yoki boshqa yo'l bilan qidiramiz
    echo "⚠️  Could not get BD_ADDR via settings, trying dumpsys..."
    BD_ADDR=$(adb shell dumpsys bluetooth_manager | grep -i "address:" | head -n 1 | awk '{print $NF}')
fi

echo "📱 Target Phone BD_ADDR: $BD_ADDR"

for i in $(seq 1 $ITERATIONS); do
    echo "[$i/$ITERATIONS] Starting reconnect test..."
    
    # vortex-l3d ni bitta ulanish urinishi bilan ishga tushiramiz
    # --max-attempts 1 bilan bitta ulanishni tekshiramiz
    VORTEX_INSECURE=1 $L3_DAEMON auto-reconnect $BD_ADDR --max-attempts 1 --fast > "${REPORTS_DIR}/vortex-l3-test-$i.log" 2>&1
    
    if grep -q "✅ attempt 1 succeeded" "${REPORTS_DIR}/vortex-l3-test-$i.log"; then
        SUCCESS=$((SUCCESS + 1))
        echo "  ✅ Connected"
        rm "${REPORTS_DIR}/vortex-l3-test-$i.log"
    else
        FAIL=$((FAIL + 1))
        echo "  ❌ Failed"
        mv "${REPORTS_DIR}/vortex-l3-test-$i.log" "${REPORTS_DIR}/failed-l3-$i.log"
    fi
    
    sleep 2
done

echo "Results: $SUCCESS/$ITERATIONS passed, $FAIL failed"
echo "| Reconnect L3 | $SUCCESS/$ITERATIONS | $FAIL | - |" >> "${SUMMARY_FILE}"
