FROM nixos/nix

RUN nix-env -i git \
  && git clone https://github.com/Shopify/comma.git \
  && nix-env -e git \
  && cd comma \
  && nix-env -i -f .

ENTRYPOINT [ "," ]
