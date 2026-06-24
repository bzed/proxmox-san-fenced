#!/bin/bash
set -e

# Create an output directory on the host
mkdir -p build-output

echo "Building the container image..."
podman build -t pve-san-fenced-builder -f Dockerfile .

echo "Running the container to build the Debian package..."
podman run --rm -v "$(pwd)/build-output:/output:z" pve-san-fenced-builder

echo "Debian package build completed. Artifacts are in $(pwd)/build-output/"
