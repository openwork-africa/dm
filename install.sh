#!/bin/sh
# Installs dm from its GitHub release into $HOME/.local/bin, without root. DM_VERSION picks a release (v0.1.0);
# DM_INSTALL_DIR another folder.
set -eu

repo=openwork-africa/dm
dir=${DM_INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)" in
  Linux) os=unknown-linux-musl ;;
  Darwin) os=apple-darwin ;;
  *) echo "dm: there is no build for $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) echo "dm: there is no build for $(uname -m)" >&2; exit 1 ;;
esac
archive="dm-$arch-$os.tar.gz"

if [ -n "${DM_VERSION:-}" ]; then
  base="https://github.com/$repo/releases/download/$DM_VERSION"
else
  base="https://github.com/$repo/releases/latest/download"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$archive" "$base/$archive"
curl -fsSL -o "$tmp/$archive.sha256" "$base/$archive.sha256"

expected=$(cut -d ' ' -f 1 < "$tmp/$archive.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$archive" | cut -d ' ' -f 1)
else
  actual=$(shasum -a 256 "$tmp/$archive" | cut -d ' ' -f 1)
fi
if [ "$expected" != "$actual" ]; then
  echo "dm: the checksum of $archive does not match; nothing was installed" >&2
  exit 1
fi

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$dir"
cp "$tmp/dm" "$dir/dm.new"
chmod 755 "$dir/dm.new"
mv "$dir/dm.new" "$dir/dm"
echo "dm installed to $dir/dm ($("$dir/dm" --version))"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH to run it as dm." ;;
esac
