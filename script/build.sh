#!/usr/bin/env bash
mkdir -p ./dist

TARGET_TRIPLE=${TARGET_TRIPLE:-x86_64-unknown-linux-gnu}
GOOS_GOARCH=${GOOS_GOARCH:-linux-amd64}

cargo build --release --locked --target "${TARGET_TRIPLE}"
mv "target/${TARGET_TRIPLE}/release/gh-jj" "./dist/gh-jj-${GOOS_GOARCH}"
