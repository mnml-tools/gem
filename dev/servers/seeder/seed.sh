#!/bin/sh
# Builds the shared test tree in the `home` volume (mounted at /home/gem in every server).
# Small fixtures are copied from /seed (dev/servers/seed, committed byte for byte) on every
# start; large or generated fixtures are created only when missing. Everything here is
# synthetic test data.
set -eu

HOME_DIR=/home/gem
GEM_UID=1000
GEM_GID=1000
LOG_SIZE=209715200 # 200 MiB: 1 638 400 lines of 128 bytes
LINES=1638400

mkdir -p "$HOME_DIR"
cd "$HOME_DIR"

# Committed fixtures (the .gitattributes that keeps them byte-exact is not part of the tree).
for dir in samples encodings; do
  rm -rf "$dir"
  cp -a "/seed/$dir" "$dir"
done

# 200 MiB log, verifiable by offset: line n (0-based) starts at byte n*128 and is
#   printf("%010d INFO  synthetic log line at offset %012d path=/api/items/%d status=200", n, n*128, n%1000)
# padded with '.' to 127 bytes, then '\n'. The integration tests rebuild any byte range from this.
mkdir -p logs
if [ "$(stat -c %s logs/app.log 2>/dev/null || echo 0)" != "$LOG_SIZE" ]; then
  echo "seed: generating logs/app.log (200 MiB)"
  awk -v lines="$LINES" 'BEGIN {
    pad = "................................................................................................................................";
    for (n = 0; n < lines; n++) {
      s = sprintf("%010d INFO  synthetic log line at offset %012d path=/api/items/%d status=200", n, n * 128, n % 1000);
      printf "%s%s\n", s, substr(pad, 1, 127 - length(s));
    }
  }' > logs/app.log.tmp
  mv logs/app.log.tmp logs/app.log
fi
printf 'short log\n' > logs/small.log

# Binary: every byte value 0..255, repeated 256 times (64 KiB), plus a 1x1 PNG.
mkdir -p binary
if [ ! -f binary/bytes.bin ]; then
  one=$(mktemp)
  i=0
  fmt=""
  while [ $i -lt 256 ]; do
    fmt="$fmt\\$(printf '%03o' $i)"
    i=$((i + 1))
  done
  printf "$fmt" > "$one"
  i=0
  while [ $i -lt 256 ]; do cat "$one"; i=$((i + 1)); done > binary/bytes.bin
  rm -f "$one"
fi
printf '\211PNG\r\n\032\n\000\000\000\rIHDR\000\000\000\001\000\000\000\001\010\006\000\000\000\037\025\304\211\000\000\000\rIDATx\234c\370\377\377?\000\005\376\002\376\247\065\201\204\000\000\000\000IEND\256B`\202' > binary/pixel.png

# A directory with 10 000 entries.
if [ ! -f many/file-10000.txt ]; then
  echo "seed: generating many/ (10 000 entries)"
  mkdir -p many
  (cd many && seq -w 1 10000 | sed 's/.*/file-&.txt/' | xargs touch)
fi

# Read-only directory: owned by root, so the gem user can neither create files in it nor
# chmod it. `locked.txt` cannot be written at all; `editable.txt` belongs to gem, so only an
# in-place overwrite works there (the non-atomic fallback).
rm -rf readonly
mkdir -p readonly
printf 'this file cannot be changed by the gem user\n' > readonly/locked.txt
printf 'editable in place, but its folder is read-only\n' > readonly/editable.txt
chown "$GEM_UID:$GEM_GID" readonly/editable.txt
chmod 0644 readonly/locked.txt readonly/editable.txt
chown root:root readonly readonly/locked.txt
chmod 0755 readonly

# Symlinks to a directory, to a file, and a dangling one.
rm -f link-to-dir link-to-file link-broken
ln -s samples link-to-dir
ln -s samples/index.php link-to-file
ln -s does-not-exist link-broken

# Names with spaces and Unicode (created here, not committed, to avoid NFC/NFD surprises).
rm -rf names
mkdir -p "names/sub dir"
printf 'space\n' > "names/with space.txt"
printf 'acentos\n' > "names/ação ünïcödé.txt"
printf 'cjk\n' > "names/日本語.txt"
printf 'emoji\n' > "names/emoji 🎉.txt"
printf 'inside\n' > "names/sub dir/inner.txt"

# Scratch space for the integration tests (each run uses its own subdirectory).
mkdir -p it

chown -R -h "$GEM_UID:$GEM_GID" samples encodings logs binary many names it link-to-dir link-to-file link-broken
chown "$GEM_UID:$GEM_GID" "$HOME_DIR"
chmod 0755 "$HOME_DIR"
# Directories created in a fresh volume can inherit the setgid bit; keep modes predictable.
find "$HOME_DIR" -type d -exec chmod g-s {} +
touch .seed-complete
echo "seed: done"
