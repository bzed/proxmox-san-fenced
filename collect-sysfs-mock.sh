#!/bin/sh
#
# collect-sysfs-mock.sh: Collect /sys/block/dm-* structure for testing sysfs traversal.
#
# Copyright (C) 2026 Bernd Zeimetz <bernd@bzed.de>
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.

set -e

TARGET_DIR=${1:-"./sys"}

echo "Collecting /sys/block/dm-* metadata into $TARGET_DIR..."
mkdir -p "$TARGET_DIR/block"

for dm_path in /sys/block/dm-*; do
    if [ ! -d "$dm_path" ]; then
        continue
    fi
    dm_name=$(basename "$dm_path")
    mkdir -p "$TARGET_DIR/block/$dm_name/dm"
    mkdir -p "$TARGET_DIR/block/$dm_name/slaves"

    if [ -f "$dm_path/dm/name" ]; then
        cp "$dm_path/dm/name" "$TARGET_DIR/block/$dm_name/dm/name"
    fi

    # Recreate the slaves directory structure with empty files
    if [ -d "$dm_path/slaves" ]; then
        for slave in "$dm_path/slaves"/*; do
            if [ -e "$slave" ]; then
                slave_name=$(basename "$slave")
                touch "$TARGET_DIR/block/$dm_name/slaves/$slave_name"
            fi
        done
    fi
done

echo "Successfully saved mock sysfs data to: $TARGET_DIR"
