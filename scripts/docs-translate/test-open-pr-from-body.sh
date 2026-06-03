#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

BODY_FILE="$TMP_DIR/body.md"
printf '## Source Pages\n\n- docs/en/reference/config.md\n' > "$BODY_FILE"
export EXPECT_HEAD="docs-translate/zh/reference-config-test"
export EXPECT_BASE="feat/docs-translate-tooling"
export EXPECT_TITLE="docs: sync zh for reference/config"
export EXPECT_BODY_FILE="$BODY_FILE"

cat > "$TMP_DIR/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

require_arg() {
    local name="$1"
    local expected="$2"
    shift 2

    while [[ $# -gt 0 ]]; do
        if [[ "$1" == "$name" ]]; then
            if [[ "${2:-}" != "$expected" ]]; then
                printf 'expected %s %s, got %s\n' "$name" "$expected" "${2:-}" >&2
                exit 1
            fi
            return 0
        fi
        shift
    done

    printf 'missing %s\n' "$name" >&2
    exit 1
}

case "$1 $2" in
    "pr list")
        require_arg --head "$EXPECT_HEAD" "$@"
        require_arg --base "$EXPECT_BASE" "$@"
        printf 'https://github.com/Cloud370/kuku/pull/99\n'
        ;;
    *)
        printf 'unexpected gh call: %s\n' "$*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$TMP_DIR/gh"

PATH="$TMP_DIR:$PATH" bash "$SCRIPT_DIR/open-pr-from-body.sh" \
    --head docs-translate/zh/reference-config-test \
    --base feat/docs-translate-tooling \
    --title 'docs: sync zh for reference/config' \
    --body-file "$BODY_FILE" > "$TMP_DIR/existing.out"

if [[ "$(<"$TMP_DIR/existing.out")" != "https://github.com/Cloud370/kuku/pull/99" ]]; then
    printf 'expected existing PR URL\n' >&2
    exit 1
fi

cat > "$TMP_DIR/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

require_arg() {
    local name="$1"
    local expected="$2"
    shift 2

    while [[ $# -gt 0 ]]; do
        if [[ "$1" == "$name" ]]; then
            if [[ "${2:-}" != "$expected" ]]; then
                printf 'expected %s %s, got %s\n' "$name" "$expected" "${2:-}" >&2
                exit 1
            fi
            return 0
        fi
        shift
    done

    printf 'missing %s\n' "$name" >&2
    exit 1
}

case "$1 $2" in
    "pr list")
        require_arg --head "$EXPECT_HEAD" "$@"
        require_arg --base "$EXPECT_BASE" "$@"
        true
        ;;
    "pr create")
        require_arg --head "$EXPECT_HEAD" "$@"
        require_arg --base "$EXPECT_BASE" "$@"
        require_arg --title "$EXPECT_TITLE" "$@"
        require_arg --body-file "$EXPECT_BODY_FILE" "$@"
        printf 'https://github.com/Cloud370/kuku/pull/100\n'
        ;;
    *)
        printf 'unexpected gh call: %s\n' "$*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$TMP_DIR/gh"

PATH="$TMP_DIR:$PATH" bash "$SCRIPT_DIR/open-pr-from-body.sh" \
    --head docs-translate/zh/reference-config-test \
    --base feat/docs-translate-tooling \
    --title 'docs: sync zh for reference/config' \
    --body-file "$BODY_FILE" > "$TMP_DIR/create.out"

if [[ "$(<"$TMP_DIR/create.out")" != "https://github.com/Cloud370/kuku/pull/100" ]]; then
    printf 'expected created PR URL\n' >&2
    exit 1
fi

cat > "$TMP_DIR/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

require_arg() {
    local name="$1"
    local expected="$2"
    shift 2

    while [[ $# -gt 0 ]]; do
        if [[ "$1" == "$name" ]]; then
            if [[ "${2:-}" != "$expected" ]]; then
                printf 'expected %s %s, got %s\n' "$name" "$expected" "${2:-}" >&2
                exit 1
            fi
            return 0
        fi
        shift
    done

    printf 'missing %s\n' "$name" >&2
    exit 1
}

case "$1 $2" in
    "pr list")
        require_arg --head "$EXPECT_HEAD" "$@"
        require_arg --base "$EXPECT_BASE" "$@"
        true
        ;;
    "pr create")
        require_arg --head "$EXPECT_HEAD" "$@"
        require_arg --base "$EXPECT_BASE" "$@"
        require_arg --title "$EXPECT_TITLE" "$@"
        require_arg --body-file "$EXPECT_BODY_FILE" "$@"
        printf 'permission denied\n' >&2
        exit 7
        ;;
    *)
        printf 'unexpected gh call: %s\n' "$*" >&2
        exit 1
        ;;
esac
EOF
chmod +x "$TMP_DIR/gh"

if PATH="$TMP_DIR:$PATH" bash "$SCRIPT_DIR/open-pr-from-body.sh" \
    --head docs-translate/zh/reference-config-test \
    --base feat/docs-translate-tooling \
    --title 'docs: sync zh for reference/config' \
    --body-file "$BODY_FILE" > "$TMP_DIR/fail.out" 2> "$TMP_DIR/fail.err"; then
    printf 'expected failed gh pr create to fail the helper\n' >&2
    exit 1
fi

printf 'open-pr-from-body tests passed\n'
