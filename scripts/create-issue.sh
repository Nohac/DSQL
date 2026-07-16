#!/usr/bin/env bash

set -euo pipefail

if [ $# -eq 0 ]; then
  echo "Usage: scripts/create-issue.sh <issue title>"
  exit 1
fi

TITLE="$*"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ISSUES_DIR="$SCRIPT_DIR/../issues"

mkdir -p "$ISSUES_DIR"

# Generate short UUID (first 8 hex chars, lowercase)
ID=$(uuidgen | tr 'A-Z' 'a-z' | tr -d '-' | cut -c1-8)

# Slugify title (lowercase, spaces -> -, remove non-alphanum)
SLUG=$(echo "$TITLE" \
  | tr 'A-Z' 'a-z' \
  | sed -E 's/[^a-z0-9]+/-/g' \
  | sed -E 's/^-+|-+$//g')

FILENAME="$ISSUES_DIR/${ID}-${SLUG}.md"

cat > "$FILENAME" <<EOF
# ${TITLE}

**ID:** ${ID} | **Status:** Open | **Created:** $(date -Iseconds)
EOF

echo "Created $FILENAME"
