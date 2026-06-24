#!/bin/bash
set -e

# Generate upstream tarball in the parent directory
tar --exclude='./debian' --exclude='./.git' --exclude='./target' -czf ../pve-san-fenced_0.1.0.orig.tar.gz .

# Build the package
dpkg-buildpackage -us -uc

# Copy all build artifacts to the mounted /output directory
if [ -d /output ]; then
    cp ../pve-san-fenced_* ../pve-san-fenced-dbgsym_* /output/
    echo "Build artifacts successfully copied to /output"
else
    echo "Warning: /output directory not found. Build artifacts are located in /build"
fi
