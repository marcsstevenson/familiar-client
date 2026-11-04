#!/usr/bin/env bash
# Fails if any Rust source in the product workspace lacks the SPDX header.
set -euo pipefail
cd "$(dirname "$0")/.."
header='// SPDX-License-Identifier: MIT OR Apache-2.0'
missing=0
while IFS= read -r -d '' f; do
  if [ "$(head -n 1 "$f")" != "$header" ]; then
    echo "missing licence header: $f"
    missing=1
  fi
done < <(find crates -name '*.rs' -not -path '*/target/*' -print0)
if [ "$missing" -ne 0 ]; then
  echo "Add this as the first line of each file above:"
  echo "  $header"
  exit 1
fi
echo "licence headers ok"
