{ lib, ... }:

# Pure helpers shared by the network/volume and project modules.
let
  r = import ../../modules/resources.nix { inherit lib; };
in
{
  tests = {
    testValidNames = {
      expr = map r.validName [
        "a"
        "A1"
        "app-net"
        "postgres_data"
        "a.b"
        "0x"
      ];
      expected = [
        true
        true
        true
        true
        true
        true
      ];
    };
    testInvalidNames = {
      expr = map r.validName [
        ""
        "-a"
        "_a"
        ".a"
        "a b"
        "a/b"
        "a:b"
      ];
      expected = [
        false
        false
        false
        false
        false
        false
        false
      ];
    };
    testNetworkNameForms = {
      expr = map r.networkName [
        "app-net"
        "app-net:alias=db"
        "app-net:ip=10.0.0.2:alias=db"
        "name=app-net"
        "name=app-net,alias=db"
        "alias=db,name=app-net"
        "driver-opt=x=y,name=app-net,alias=db"
        "host"
      ];
      expected = [
        "app-net"
        "app-net"
        "app-net"
        "app-net"
        "app-net"
        "app-net"
        "app-net"
        "host"
      ];
    };
    testNetworkNameDoesNotMistakeAnAliasForTheName = {
      expr = r.networkName "app-net:name=other";
      expected = "app-net";
    };
    testVolumeSource = {
      expr = map r.volumeSource [
        "data:/d"
        "data:/d:ro,Z"
        "/srv:/srv"
        "./rel:/r"
        "bare"
      ];
      expected = [
        "data"
        "data"
        "/srv"
        "./rel"
        "bare"
      ];
    };
    testNetworksFromExtraOptions = {
      expr = r.networksFromExtraOptions [
        "--cap-add=NET_ADMIN"
        "--network=a"
        "--net=b:alias=x"
        "--network"
        "c"
        "--net"
        "name=d,alias=y"
        "--network=name=e,alias=z"
        "--init"
      ];
      expected = [
        "a"
        "b"
        "c"
        "d"
        "e"
      ];
    };
    testNetworksFromExtraOptionsEdgeCases = {
      expr = {
        empty = r.networksFromExtraOptions [ ];
        danglingFlag = r.networksFromExtraOptions [ "--network" ];
        unrelated = r.networksFromExtraOptions [
          "--hostname=h"
          "--networking"
        ];
      };
      expected = {
        empty = [ ];
        danglingFlag = [ ];
        unrelated = [ ];
      };
    };
  };
}
