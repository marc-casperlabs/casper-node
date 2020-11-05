{ pkgs ? (import <nixpkgs>) { } }:
let
  aliasHelper = pkgs.writeTextFile {
    name = "alias-helper";
    text = ''
      #!${pkgs.python3}/bin/python

      import os
      import shlex
      import sys

      INPUT=sys.argv[1]
      OUTPUT_DIR=sys.argv[2]
      BASH=sys.argv[3]

      for line in open(INPUT):
          line = line.strip()
          if not line.startswith('alias'):
              continue
          alias, cmd_escaped = line.lstrip('alias').strip().split('=', 1)
          cmd = shlex.split(cmd_escaped)[0]
          out_fn = os.path.join(OUTPUT_DIR, alias)

          print("writing " + out_fn)
          open(out_fn, 'w').write("#!{}\n\nset -e\n\n{}\n".format(BASH, cmd))
          os.chmod(out_fn, 0o0555)
    '';
    executable = true;
    destination = "/bin/alias-helper";
  };

  nctlLib = pkgs.stdenv.mkDerivation {
    name = "nctl";
    src = ./utils/nctl;

    buildPhase = ''
      # Compile markdown manpages
      ${pkgs.ronn}/bin/ronn --organization="CasperLabs LLC" --roff --pipe $src/README.md | ${pkgs.gzip}/bin/gzip -9 > nctl.1.gz

      for md in $src/docs/*.md; do
        ${pkgs.ronn}/bin/ronn --organization="CasperLabs LLC" --roff --pipe $md | ${pkgs.gzip}/bin/gzip -9 > nctl-$(basename ''${md%.*}).1.gz
      done;

      # Turn aliases into proper commands
      mkdir commands
      ${aliasHelper}/bin/alias-helper "$src/sh/utils/set_aliases.sh" "commands" ${pkgs.bash}/bin/bash
      ls -ld commands
    '';

    installPhase = ''
      mkdir -p $out/share/man/man1 $out/lib/nctl $out/lib/nctl-commands
      cp -rv $src/* $out/lib/nctl/
      cp -v *.1.gz $out/share/man/man1/
      cp -v commands/* $out/lib/nctl-commands
    '';
  };

  binScript = pkgs.writeShellScriptBin "nctl" ''
    #!${pkgs.bash}/bin/bash

    set -e

    # TODO: Maybe not enumerate build-inputs manually here?
    export PATH=${pkgs.jq}/bin:${pkgs.bash}/bin:${pkgs.gnumake}/bin:${pkgs.python3}/bin:''${PATH}

    if [ $# -lt 1 ]; then
      echo "usage: nctl COMMAND [ARGS]"
      echo "Try \`nctl help commands\` for an overview of commands"
      exit 1;
    fi

    # New command help launches man with the respective command.
    if [ $1 = "help" ]; then
      if [ $# -eq 1 ]; then
        TARGET=nctl
      else
        TARGET=nctl-$2
      fi;

      man ${nctlLib}/share/man/man1/$TARGET.1.gz
      exit 0;
    fi;

    # We're installing in a stand-alone package, so disable the magic that sets the home dir here.
    # Our shell derivation will re-set it.
    if [ -z ''${NCTL_CASPER_HOME} ]; then
      echo NCTL_CASPER_HOME is not set. Please set it in your environment before calling nctl.
      exit 1;
    fi

    CMD=nctl-$*
    OLD_HOME=''${NCTL_CASPER_HOME}

    . ${nctlLib}/lib/nctl/activate
    export NCTL_CASPER_HOME=''${OLD_HOME}

    CMD="${nctlLib}/lib/nctl-commands/nctl-$*"
    exec $CMD
  '';
in binScript
