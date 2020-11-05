#!/bin/sh

set -eu

export NCTL_CASPER_HOME=$(pwd)

nix-shell -E 'with import <nixpkgs> { }; runCommand "dummy" { buildInputs = [ (import ./nctl.nix {}) ]; } ""'
