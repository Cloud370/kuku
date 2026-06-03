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
    "- Tier: `" + (value("tier"; "unknown") | tostring) + "`",
    "- Model: `" + (value("model"; "unknown") | tostring) + "`",
    "- Turns: `" + ((value("turns"; 0)) | tostring) + "`",
    "- Input tokens: `" + ((value("input_tokens"; 0)) | tostring) + "`",
    "- Output tokens: `" + ((value("output_tokens"; 0)) | tostring) + "`",
    "- Cache read tokens: `" + ((value("cache_read_input_tokens"; 0)) | tostring) + "`",
    "- Cache write tokens: `" + ((value("cache_creation_input_tokens"; 0)) | tostring) + "`",
    "- Duration: `" + fmt_duration + "`"
  ]
  | .[]
' "$SUMMARY_JSON"
