# Fioxa

# Setup

1. Ensure qemu in installed
2. Get a copy of OVMF-pure-efi.fd and copy it into builder/ovmf/OVMF-pure-efi.fd (you can get such a file from https://github.com/rust-osdev/ovmf-prebuilt/releases).
    * If you are on a linux host, auto detection with try and use the system installed OVMF. 

## Building

In the builder folder run `cargo run`.

To also launch qemu run `cargo run qemu`

### Build image

This only works on a linux host.

```sh
dd if=/dev/zero of=fioxa.img bs=100M count=1
mformat -i fioxa.img ::
mcopy -s -i fioxa.img fioxa/* ::
```

### Convert image to VDI

```sh
VBoxManage convertfromraw fioxa.img fioxa.vdi
```

## Debugging

Ensure that wait-port is installed (https://www.npmjs.com/package/wait-port).
Use the VSCode "Build & Launch Kernel" debug target.