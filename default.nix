{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "muserial";
  version = "0.0.0";
  src = ./.;
  cargoSha256 = "Kkma6J56toTnboTpYWhvK6qrgMveSsEP9hfU6PbLNzs=";
}
