#!/bin/sh
name=comva
target=x86_64-unknown-linux-gnu
path=target/$target/release/$name
echo "Building.."
# Use nightly custom build settings
cargo build -q --target $target --release
strip $path
echo "Build size: $(ls -la $path | awk '{print $5}') bytes " 
# Install the binary
cp $path ~/.local/bin/$name
