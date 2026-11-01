#!/bin/sh
# Installs the latest baton release binary into $BATON_INSTALL_DIR
# (default ~/.local/bin), after checking its SHA-256.
#   curl -fsSL https://raw.githubusercontent.com/macalinao/batonpass/master/install.sh | sh
set -eu

case "$(uname -s)-$(uname -m)" in
Linux-x86_64) target=x86_64-unknown-linux-musl ;;
Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
Darwin-x86_64) target=x86_64-apple-darwin ;;
Darwin-arm64) target=aarch64-apple-darwin ;;
*)
	echo "baton: no prebuilt binary for $(uname -s) $(uname -m); try cargo install batonpass" >&2
	exit 1
	;;
esac

url="https://github.com/macalinao/batonpass/releases/latest/download/baton-$target.tar.gz"
dir="${BATON_INSTALL_DIR:-$HOME/.local/bin}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

cd "$tmp"
curl -fsSLO "$url"
curl -fsSLO "$url.sha256"
if command -v sha256sum >/dev/null; then
	sha256sum -c "baton-$target.tar.gz.sha256"
else
	shasum -a 256 -c "baton-$target.tar.gz.sha256"
fi
tar -xzf "baton-$target.tar.gz" baton
mkdir -p "$dir"
mv baton "$dir/baton"
echo "installed $dir/baton"
