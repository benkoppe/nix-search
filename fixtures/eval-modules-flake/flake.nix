{
  description = "nix-search eval-modules fixture";

  outputs =
    { self }:
    {
      nixosModules.default = ./module.nix;
    };
}
