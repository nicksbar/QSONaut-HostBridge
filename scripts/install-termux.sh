#!/data/data/com.termux/files/usr/bin/bash
set -euo pipefail

repo_dir="${QSONAUT_HOSTBRIDGE_DIR:-$HOME/QSONaut-HostBridge}"
install_dir="${PREFIX:-$HOME/.local}/bin"

pkg update -y
pkg install -y clang git rust

if command -v rustup >/dev/null 2>&1; then
    rustup target add aarch64-linux-android
fi

if [ ! -d "$repo_dir/.git" ]; then
    git clone https://github.com/nicksbar/QSONaut-HostBridge "$repo_dir"
fi

cd "$repo_dir"
git pull --ff-only
cargo build --locked --release --bin qsonaut-hostbridge
mkdir -p "$install_dir"
cp target/release/qsonaut-hostbridge "$install_dir/qsonaut-hostbridge"
chmod 755 "$install_dir/qsonaut-hostbridge"

echo "Installed $install_dir/qsonaut-hostbridge"
echo "Run: $install_dir/qsonaut-hostbridge config set --bind 0.0.0.0:8765"
