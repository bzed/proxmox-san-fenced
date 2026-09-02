#!/bin/bash
set -e

# Build the package (native format — no orig.tar.gz needed)
dpkg-buildpackage -us -uc

# Copy build artifacts to the mounted /output directory
if [ -d /output ]; then
    cp ../pve-san-fenced_* /output/ 2>/dev/null || true
    # Copy dbgsym artifacts if they exist
    cp ../pve-san-fenced-dbgsym_* /output/ 2>/dev/null || true
    echo "Build artifacts successfully copied to /output"
else
    echo "Warning: /output directory not found. Build artifacts are located in /build"
fi
