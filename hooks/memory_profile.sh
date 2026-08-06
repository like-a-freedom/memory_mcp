#!/usr/bin/env bash
# Samples RSS (and footprint when available) of a running memory_mcp process.
# Usage: hooks/memory_profile.sh <pid> <duration_secs> [log_file]
set -euo pipefail

pid="${1:?usage: hooks/memory_profile.sh <pid> <duration_secs> [log_file]}"
duration="${2:?}"
log="${3:-/tmp/memory_mcp_rss.log}"

peak=0
start=$(date +%s)
: > "$log"
while (( $(date +%s) - start < duration )); do
  rss_kb=$(ps -o rss= -p "$pid" | tr -d ' ' || echo "0")
  fp=$(footprint "$pid" 2>/dev/null | awk '/Physical footprint/ {print $3}' | head -1 || echo "")
  if (( rss_kb > peak )); then peak=$rss_kb; fi
  echo "$(date +%H:%M:%S) rss_kb=$rss_kb footprint=$fp" >> "$log"
  sleep 2
done
echo "PEAK_RSS_KB=$peak" | tee -a "$log"
