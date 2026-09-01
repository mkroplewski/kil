#!/bin/sh
set -eu

repository="mkroplewski/kil"
install_root="${KIL_INSTALL_ROOT:-$HOME/.local/share/kil}"
bin_dir="${KIL_BIN_DIR:-$HOME/.local/bin}"
release_base="https://github.com/$repository/releases/latest/download"

case "$install_root" in
    /*) ;;
    *)
        echo "KIL_INSTALL_ROOT must be an absolute path" >&2
        exit 1
        ;;
esac
case "$bin_dir" in
    /*) ;;
    *)
        echo "KIL_BIN_DIR must be an absolute path" >&2
        exit 1
        ;;
esac

mkdir -p "$install_root" "$bin_dir"
install_root=$(cd -P "$install_root" && pwd)
bin_dir=$(cd -P "$bin_dir" && pwd)
home_dir=$(cd -P "$HOME" && pwd)
case "$install_root" in
    /|"$home_dir")
        echo "KIL_INSTALL_ROOT must be a dedicated subdirectory" >&2
        exit 1
        ;;
esac

os=$(uname -s)
arch=$(uname -m)

case "$os:$arch" in
    Linux:x86_64|Linux:amd64)
        target="x86_64-unknown-linux-gnu"
        ;;
    Darwin:x86_64|Darwin:amd64)
        target="x86_64-apple-darwin"
        ;;
    Darwin:arm64|Darwin:aarch64)
        target="aarch64-apple-darwin"
        ;;
    *)
        echo "kil has no prebuilt binary for $os $arch" >&2
        exit 1
        ;;
esac

asset="kil-$target.tar.gz"
temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/kil-install.XXXXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT HUP INT TERM

curl -fLsS "$release_base/$asset" -o "$temporary_dir/$asset"
curl -fLsS "$release_base/SHA256SUMS" -o "$temporary_dir/SHA256SUMS"

expected=$(awk -v asset="$asset" '$2 == asset { print $1; exit }' "$temporary_dir/SHA256SUMS")
if [ -z "$expected" ]; then
    echo "release checksum for $asset was not found" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$temporary_dir/$asset" | awk '{ print $1 }')
else
    actual=$(shasum -a 256 "$temporary_dir/$asset" | awk '{ print $1 }')
fi

if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $asset" >&2
    exit 1
fi

mkdir "$temporary_dir/payload"
tar -xzf "$temporary_dir/$asset" -C "$temporary_dir/payload"
if [ ! -f "$temporary_dir/payload/kil" ] || [ ! -f "$temporary_dir/payload/krt/py_router/route.py" ]; then
    echo "release archive is missing kil or KiCadRoutingTools" >&2
    exit 1
fi

python=""
for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 &&
       "$candidate" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 9) else 1)' >/dev/null 2>&1; then
        python="$candidate"
        break
    fi
done
if [ -z "$python" ]; then
    echo "Python 3.9 or newer is required for the bundled autorouter" >&2
    exit 1
fi

mkdir -p "$install_root/bin" "$install_root/lib"
installed_binary="$install_root/bin/kil"
launcher="$bin_dir/kil"
install -m 755 "$temporary_dir/payload/kil" "$installed_binary"
if [ "$launcher" != "$installed_binary" ]; then
    if [ -d "$launcher" ]; then
        echo "$launcher is a directory; cannot install kil command" >&2
        exit 1
    fi
    rm -f "$launcher"
    ln -s "$installed_binary" "$launcher"
fi
rm -rf "$install_root/lib/krt"
cp -R "$temporary_dir/payload/krt" "$install_root/lib/krt"

if [ ! -x "$install_root/python/bin/python" ]; then
    "$python" -m venv "$install_root/python"
fi
"$install_root/python/bin/python" -m pip install --disable-pip-version-check --quiet --upgrade \
    -r "$install_root/lib/krt/requirements.txt"

echo "Installed kil and KiCadRoutingTools to $install_root"
case ":$PATH:" in
    *":$bin_dir:"*) echo "Run: kil --version" ;;
    *) echo "Add $bin_dir to PATH, then run: kil --version" ;;
esac
