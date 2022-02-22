#!/bin/sh
name=comva
target=x86_64-unknown-linux-gnu
path=target/$target/debug/$name
echo "Building (debug).."
cargo build --target $target -q
# Install the binary
cp $path ~/.local/bin/$name
echo "Done."
