#!/bin/sh
# tools/pts-profile/setup.sh
#
# One-shot setup script for pts/sqlite benchmarking of the liter-rs project.
#
# Steps performed:
#   1. Install Phoronix Test Suite (via apt) if not already present.
#   2. Copy local test profiles into ~/.phoronix-test-suite/test-profiles/pts/.
#   3. Print the commands needed to install and run each benchmark.
#
# Usage (from the workspace root):
#   bash tools/pts-profile/setup.sh
#
# Requirements:
#   - Debian/Ubuntu with apt (or an existing phoronix-test-suite on PATH)
#   - PHP 8.x (php-cli + php-xml) — pulled in by PTS
#   - Rust toolchain with `cargo` on PATH (already in this dev container)
#   - build-essential / cc for compiling liter-ffi-cli

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROFILE_SRC="$SCRIPT_DIR/pts"
PTS_PROFILE_DIR="$HOME/.phoronix-test-suite/test-profiles/pts"

# ---------------------------------------------------------------------------
# 1. Install Phoronix Test Suite
# ---------------------------------------------------------------------------
if ! command -v phoronix-test-suite > /dev/null 2>&1; then
    echo "==> Installing PHP (required by PTS)..."
    sudo apt-get update -qq
    sudo apt-get install -y php-cli php-xml

    # phoronix-test-suite is not in Debian's default repos; install from
    # the upstream GitHub release tarball.
    PTS_VER="10.8.4"
    PTS_TGZ="phoronix-test-suite-${PTS_VER}.tar.gz"
    PTS_URL="https://github.com/phoronix-test-suite/phoronix-test-suite/releases/download/v${PTS_VER}/${PTS_TGZ}"

    echo "==> Downloading Phoronix Test Suite v${PTS_VER}..."
    curl -fsSL "$PTS_URL" -o "/tmp/${PTS_TGZ}"
    tar -xzf "/tmp/${PTS_TGZ}" -C /tmp
    # The tarball extracts to phoronix-test-suite/ (without the version suffix).
    echo "==> Installing Phoronix Test Suite..."
    (cd /tmp/phoronix-test-suite && sudo ./install-sh)
    rm -rf /tmp/phoronix-test-suite "/tmp/${PTS_TGZ}"
else
    echo "==> phoronix-test-suite already installed: $(phoronix-test-suite version 2>/dev/null | head -1)"
fi

# ---------------------------------------------------------------------------
# 2. Copy local profiles
# ---------------------------------------------------------------------------
echo "==> Copying test profiles to $PTS_PROFILE_DIR"
mkdir -p "$PTS_PROFILE_DIR"
cp -r "$PROFILE_SRC/"* "$PTS_PROFILE_DIR/"

# Make shell scripts executable inside the installed profiles.
find "$PTS_PROFILE_DIR" -name "*.sh" -exec chmod +x {} \;

echo "==> Profiles installed:"
ls "$PTS_PROFILE_DIR/"

# ---------------------------------------------------------------------------
# 3. Print next steps
# ---------------------------------------------------------------------------
cat << 'EOF'

=== Next Steps ===

Install and run the C reference benchmark (builds SQLite 3.53.0 from source):
  phoronix-test-suite install pts/sqlite-2.3.0
  phoronix-test-suite run     pts/sqlite-2.3.0

Install and run the liter-shell benchmark:
  phoronix-test-suite install pts/liter-sqlite-1.0.0
  phoronix-test-suite run     pts/liter-sqlite-1.0.0

Install and run the liter-ffi benchmark:
  phoronix-test-suite install pts/liter-ffi-sqlite-1.0.0
  phoronix-test-suite run     pts/liter-ffi-sqlite-1.0.0

Run all three and compare results:
  phoronix-test-suite benchmark pts/sqlite-2.3.0 pts/liter-sqlite-1.0.0 pts/liter-ffi-sqlite-1.0.0

NOTE: pts/sqlite-2.3.0 downloads sqlite-autoconf-3530000.tar.gz from
sqlite.org.  If the download fails, verify the release year in
tools/pts-profile/pts/sqlite-2.3.0/downloads.xml and update the URL
(e.g. change /2026/ to /2025/).

EOF
