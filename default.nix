{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "muserial";
  version = "0.0.0";
  src = ./.;
  cargoSha256 = "5IrcK79zwf1C9/HD1utqWGkQfn88Kqfbcalt1/atwRU=";
}
