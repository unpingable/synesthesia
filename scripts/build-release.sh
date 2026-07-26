#!/usr/bin/env bash
set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
if [[ $PWD != "$repository_root" ]]; then
  echo "run scripts/build-release.sh from the repository root" >&2
  exit 1
fi

if [[ ${SYNESTHESIA_ALLOW_DIRTY:-0} != 1 ]] && [[ -n $(git status --porcelain) ]]; then
  echo "refusing to package a dirty worktree; commit first or set SYNESTHESIA_ALLOW_DIRTY=1" >&2
  exit 1
fi

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
  echo "this release path supports only x86_64 Linux" >&2
  exit 1
fi

version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
if [[ -z $version ]]; then
  echo "could not read the package version from Cargo.toml" >&2
  exit 1
fi
if [[ ${GITHUB_REF_TYPE:-} == tag && ${GITHUB_REF_NAME:-} != "v$version" ]]; then
  echo "tag ${GITHUB_REF_NAME:-<missing>} does not match Cargo version $version" >&2
  exit 1
fi

target_triple=x86_64-unknown-linux-gnu
archive_name="synesthesia-v${version}-${target_triple}.tar.gz"
target_dir=${CARGO_TARGET_DIR:-target}
target_dir_absolute=$(realpath -m "$target_dir")
if [[ $target_dir_absolute == / || $target_dir_absolute == "$repository_root" ]]; then
  echo "refusing unsafe CARGO_TARGET_DIR: $target_dir" >&2
  exit 1
fi
dist_dir="$target_dir/dist"
stage_dir="$dist_dir/stage"
archive_path="$dist_dir/$archive_name"
checksum_path="$archive_path.sha256"

export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
if [[ ! $SOURCE_DATE_EPOCH =~ ^[0-9]+$ ]]; then
  echo "SOURCE_DATE_EPOCH must be a non-negative integer" >&2
  exit 1
fi

release_toolchain=${SYNESTHESIA_RELEASE_TOOLCHAIN:-1.85}
release_rustc=$(rustup run "$release_toolchain" rustc --version)
if [[ $release_rustc != "rustc 1.85.1 "* ]]; then
  echo "release builds require rustc 1.85.1; got $release_rustc" >&2
  exit 1
fi
cargo_home=${CARGO_HOME:-${HOME:?HOME must be set}/.cargo}
release_rustflags="${RUSTFLAGS:-} --remap-path-prefix=$repository_root=/src/synesthesia"
release_rustflags+=" --remap-path-prefix=$cargo_home/registry/src=/cargo/registry/src"
RUSTFLAGS="$release_rustflags" \
  rustup run "$release_toolchain" cargo build --locked --release --features ebpf --bins

rm -rf "$stage_dir"
rm -f "$archive_path" "$checksum_path"
mkdir -p "$stage_dir"

install -m 0755 "$target_dir/release/synesthesia" "$stage_dir/synesthesia"
install -m 0755 \
  "$target_dir/release/synesthesia-scheduler-collector" \
  "$stage_dir/synesthesia-scheduler-collector"
install -m 0755 \
  "$target_dir/release/synesthesia-tcp-collector" \
  "$stage_dir/synesthesia-tcp-collector"
install -m 0644 README.md "$stage_dir/README.md"
install -m 0644 RELEASE_NOTES.md "$stage_dir/RELEASE_NOTES.md"
install -m 0644 LICENSE "$stage_dir/LICENSE"
install -m 0644 NOTICE "$stage_dir/NOTICE"

find "$stage_dir" -exec touch -d "@$SOURCE_DATE_EPOCH" {} +

tar \
  --sort=name \
  --format=gnu \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --mtime="@$SOURCE_DATE_EPOCH" \
  --mode='u+rwX,go+rX,go-w' \
  -C "$stage_dir" \
  -cf - . |
  gzip -n >"$archive_path"

(
  cd "$dist_dir"
  sha256sum "$archive_name" >"$archive_name.sha256"
)

printf 'archive: %s\nchecksum: %s\n' "$archive_path" "$checksum_path"
