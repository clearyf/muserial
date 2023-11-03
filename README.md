# Overview

This is a very basic serial port communication program.  Currently supported:

- 115200 baud, no parity.
- Logfile support.  Everything received from the Uart is logged
  directly into a file.  The location of the files only configurable
  by adjusting the source code.  If muserial cleanly exits then the
  logfile will be compressed with xz to save space.

There used to be other features to add/remove newlines, linefeed, etc,
but I removed them as I was not using them any more.

# Usage

Exit by hitting `Ctrl-o`.

# Building

Either get cargo/rustc from somewhere (your distro, etc), or use the
provided `default.nix` to build using nix.

Create `~/.config/nixpkgs/overlays/muserial.nix` with the following
contents, remember to replace `PATH_TO_MUSERIAL_REPO` with the actual
path to this repo.

```
self: super: {
  muserial = (super.callPackage /PATH_TO_MUSERIAL_REPO {}) {};
}
```

Install using `nix-env -iA nixpkgs.muserial`.
