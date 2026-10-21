#!/bin/sh
# Hash top-level stock-card .dtb files. Read-only.
# Skips files larger than 1 MiB. Opens at most 64 files.
# Reading a mounted FAT volume may update access times (metadata only).
set -eu

MAX_BYTES=1048576
MAX_FILES=64

if [ "$#" -ne 1 ]; then
  echo "usage: hash-dtb.sh FILE_OR_BOOT_DIR" >&2
  exit 2
fi

hash_file() {
  file=$1
  size=$(wc -c < "$file" | tr -d '[:space:]')
  if [ "$size" -gt "$MAX_BYTES" ]; then
    echo "skip (>1 MiB): $file" >&2
    return 0
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$file"
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -- "$file"
  else
    echo "need sha256sum or shasum" >&2
    exit 1
  fi
}

target=$1
if [ -f "$target" ]; then
  hash_file "$target"
  exit 0
fi
if [ ! -d "$target" ]; then
  echo "not a file or directory: $target" >&2
  exit 1
fi

count=0
files=$(find "$target" -maxdepth 1 -type f -name '*.dtb' | sort)
printf '%s\n' "$files" | while IFS= read -r file; do
  [ -n "$file" ] || continue
  if [ "$count" -ge "$MAX_FILES" ]; then
    break
  fi
  size=$(wc -c < "$file" | tr -d '[:space:]')
  if [ "$size" -gt "$MAX_BYTES" ]; then
    echo "skip (>1 MiB): $file" >&2
    continue
  fi
  count=$((count + 1))
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -- "$file"
  else
    shasum -a 256 -- "$file"
  fi
done
