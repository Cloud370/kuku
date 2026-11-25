#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'usage: %s <session-run-json-path>\n' "${0##*/}" >&2
    exit 1
fi

SUMMARY_JSON="$1"

if [[ ! -f "$SUMMARY_JSON" ]]; then
    printf '### Run stats\n\n- Run stats unavailable\n'
    exit 0
fi

jq -r '
  def value(name; fallback): .[name] // fallback;
  def usage(name): .usage[name] // "unreported";
  def fmt_duration:
    (value("duration_ms"; 0)) as $ms
    | if $ms >= 1000
      then (((($ms / 100) | floor) / 10) | tostring) + "s"
      else ($ms | tostring) + "ms"
      end;
  [
    "### Run stats",
    "",
    "- Session: `" + (value("session_id"; "unknown") | tostring) + "`",
    "- Connection: `" + (value("connection"; "unknown") | tostring) + "`",
    "- Model: `" + (value("model"; "unknown") | tostring) + "`",
    "- Turns: `" + ((value("turns"; 0)) | tostring) + "`",
    "- Input tokens: `" + (usage("input_tokens") | tostring) + "`",
    "- Output tokens: `" + (usage("output_tokens") | tostring) + "`",
    "- Cache read tokens: `" + (usage("cache_read_input_tokens") | tostring) + "`",
    "- Cache write tokens: `" + (usage("cache_creation_input_tokens") | tostring) + "`",
    "- Duration: `" + fmt_duration + "`"
  ]
  | .[]
' "$SUMMARY_JSON"
