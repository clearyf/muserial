{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "muserial";
  version = "0.0.0";
  src = ./.;
  cargoSha256 = "vv7vs79ALjbG97QLlJiXtm6tKS/A5dNE3QOVYq9plKU=";
}
