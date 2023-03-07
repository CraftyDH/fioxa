use std::{
    env::args,
    fs::{copy, DirBuilder},
    io::BufReader,
    process::{Command, Stdio},
};

use cargo_metadata::{camino::Utf8PathBuf, Message};

fn main() {
    let mut dirs = DirBuilder::new();

    let bootloader = build("bootloader").unwrap();
    let kernel = build("kernel").unwrap();
    let elf = build("test_elf").unwrap();
    let calc = build("calc").unwrap();

    dirs.recursive(true).create("fioxa/EFI/BOOT").unwrap();
    copy("assets/startup.nsh", "fioxa/startup.nsh").unwrap();
    copy("assets/zap-light16.psf", "fioxa/font.psf").unwrap();
    copy(bootloader, "fioxa/EFI/BOOT/BOOTx64.efi").unwrap();
    copy(kernel, "fioxa/fioxa.elf").unwrap();
    copy(elf, "fioxa/elf.elf").unwrap();
    copy(calc, "fioxa/calc.elf").unwrap();

    let mut args = args();

    if args.any(|a| a == "qemu") {
        Command::new("qemu-system-x86_64")
            .args([
                // GDB server
                "-s",
                "-S",
                // Args
                "-machine",
                "q35",
                // "-no-shutdown",
                // "-no-reboot",
                "-cpu",
                "qemu64",
                "-smp",
                // "cores=12",
                "cores=4",
                "-m",
                "512M",
                "-serial",
                "stdio",
                "-drive",
                "if=pflash,format=raw,file=ovmf/OVMF-pure-efi.fd",
                "-drive",
                "format=raw,file=fat:rw:fioxa",
                "-drive",
                "format=raw,file=fat:rw:src",
            ])
            .spawn()
            .unwrap();
    }
}

fn build(name: &str) -> Result<Utf8PathBuf, String> {
    // Build subprocess
    let mut cargo = Command::new("cargo")
        .current_dir(format!("../{}", name))
        .args([
            "build",
            // "--release",
            "--message-format=json-render-diagnostics",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    // Grab stout and read it
    let reader = BufReader::new(cargo.stdout.take().unwrap());
    let mut path: Option<Utf8PathBuf> = None;
    for message in Message::parse_stream(reader) {
        match message.unwrap() {
            // print messages to user
            Message::CompilerMessage(msg) => println!("{}", msg),

            Message::CompilerArtifact(artifact) if artifact.target.name == name => {
                // Save the path
                path = artifact.executable
            }
            Message::BuildFinished(finished) => {
                // Successful build return path of executable
                if finished.success {
                    return Ok(path.expect(
                        format!("No executable found in artifact for: {}", name).as_str(),
                    ));
                // Didn't build correctly :(
                } else {
                    return Err(format!("Failed build of: {}", name));
                }
            }
            // Ignore other messages
            _ => {}
        }
    }
    Err(format!("Unexpected error for package: {}", name))
}
