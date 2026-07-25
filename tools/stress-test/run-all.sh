#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
REPORTS_DIR="${SCRIPT_DIR}/reports"
mkdir -p "${REPORTS_DIR}"

SUMMARY_FILE="${REPORTS_DIR}/summary.md"

echo "🚀 Starting Vortex Stress Test Suite..."
echo "# Stress Test Report" > "${SUMMARY_FILE}"
echo "Date: $(date)" >> "${SUMMARY_FILE}"
echo "" >> "${SUMMARY_FILE}"
echo "## Results" >> "${SUMMARY_FILE}"
echo "| Scenario | Pass | Fail | Rate |" >> "${SUMMARY_FILE}"
echo "|---|---|---|---|" >> "${SUMMARY_FILE}"

for scenario in "${SCRIPT_DIR}/scenarios"/*.sh; do
    if [ -f "$scenario" ] && [ -x "$scenario" ]; then
        echo "▶️ Running scenario: $(basename "$scenario")"
        "$scenario"
    fi
done

echo "🎉 All tests completed. Check ${SUMMARY_FILE}."
