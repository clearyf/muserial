{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "muserial";
  version = "0.0.1";
  src = ./.;
  cargoSha256 = "180q2xqp8z77m8dh5frcimg64dwzbzb4rmw3nxd5vgc4q2ag1k9k";
}
