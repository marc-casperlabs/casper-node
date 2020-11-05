#!/usr/bin/env bash
#
# List previously created assets.
# Globals:
#   NCTL - path to nctl home directory.

if [ -d $NCTL_DATA/assets ]; then
    ls $NCTL_DATA/assets
fi
