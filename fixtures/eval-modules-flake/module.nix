{ lib, ... }:
{
  options.programs.fixture.enable = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = "Whether to enable the fixture program.";
  };
}
