{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "muserial";
  version = "0.0.0";
  src = ./.;
  cargoSha256 = "Fn0eqVj3MZjBUn7tgtIZb+vIIC/2fQLJOz9T78DUGSM=";
}
