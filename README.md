# Overview

This is a very basic serial port communication program.  Currently supported:

- Fixed 115200 baud
- Optional local echo (`--local-echo`)
- Optional CR/NL translation (`--crnl-translation`)

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
