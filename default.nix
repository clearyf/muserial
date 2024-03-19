{ rustPlatform }:

rustPlatform.buildRustPackage rec {
  pname = "muserial";
  version = "0.0.0";
  src = ./.;
  cargoHash = "sha256-2dF7zzGzMpWOKHiewiGphItVjNthmOGKjzP2QOALoE4=";
}
