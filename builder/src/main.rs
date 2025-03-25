use std::{
    env::{self, args},
    fs::{self, DirBuilder, copy},
    io::BufReader,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result};
use cargo_metadata::{Message, camino::Utf8PathBuf};
use errors::BuildErrors;

use crate::errors::QEMUErrors;

pub mod errors;

const PURE_EFI_PATH: &'static str = "ovmf/OVMF-pure-efi.fd";
const LOCAL_EFI_VARS: &'static str = "ovmf/VARS.fd";
const SYSTEM_EFI_CODE: &'static str = "/usr/share/OVMF/OVMF_CODE.fd";
const SYSTEM_EFI_VARS: &'static str = "/usr/share/OVMF/OVMF_VARS.fd";

const TO_BUILD: &[(&'static str, &'static str)] = &[
    ("bootloader", "EFI/BOOT/BOOTx64.efi"),
    ("test_elf", "elf.elf"),
    ("amd_pcnet", "amd_pcnet.driver"),
    ("calc", "calc.elf"),
    ("net", "net.elf"),
    ("ps2", "ps2.driver"),
    ("terminal", "terminal.elf"),
    // ! MUST BE LAST
    ("kernel", "fioxa.elf"),
];

fn main() -> Result<()> {
    if args().any(|a| a == "clean") {
        for (package, _) in TO_BUILD {
            clean(package)?;
        }
        return Ok(());
    }

    let mut dirs = DirBuilder::new();

    dirs.recursive(true).create("fioxa/EFI/BOOT")?;
    copy("assets/startup.nsh", "fioxa/startup.nsh")?;
    copy("assets/zap-light16.psf", "fioxa/font.psf")?;

    let release = args().any(|a| a == "--release");

    for (package, out) in TO_BUILD {
        let exec_path =
            build(package, release).with_context(|| format!("Failed to build {}", package))?;
        copy(exec_path, format!("fioxa/{}", out)).with_context(|| {
            format!("Failed to copy the output of {} to fioxa/{}", package, out)
        })?;
    }

    if args().any(|a| a == "qemu") {
        qemu(true).context("Failed to launch qemu")?;
    } else if args().any(|a| a == "qemu-nox") {
        qemu(false).context("Failed to launch qemu")?;
    }

    Ok(())
}

/// **Warning:** Contains intentional memory leaks, because I am lazy
fn qemu(with_screen: bool) -> Result<()> {
    let mut qemu_args = vec![
        // GDB server
        "-s".into(),
        // Args
        "-machine".into(),
        "q35".into(),
        "-cpu".into(),
        "qemu64".into(),
        "-smp".into(),
        "cores=4".into(),
        "-m".into(),
        "512M".into(),
        "-netdev".into(),
        "user,id=mynet0".into(),
        "-device".into(),
        "pcnet,netdev=mynet0,mac=00:11:22:33:44:55".into(),
        // Log network trafic
        "-object".into(),
        "filter-dump,id=id,netdev=mynet0,file=fioxa.pcap".into(),
    ];

    if has_kvm() {
        qemu_args.push("-enable-kvm".to_string());
    }

    if with_screen {
        qemu_args.push("-serial".into());
        qemu_args.push("stdio".into());
    } else {
        qemu_args.push("-nographic".into());
    }

    // add any args to qemu after --
    qemu_args.extend(args().skip_while(|a| a != "--").skip(1));

    let pure_path = Path::new(PURE_EFI_PATH);
    let local_vars = Path::new(LOCAL_EFI_VARS);
    let system_code = Path::new(SYSTEM_EFI_CODE);
    let system_vars = Path::new(SYSTEM_EFI_VARS);

    let ovmf_folder = Path::new("./ovmf");
    if !ovmf_folder.exists() {
        fs::create_dir("ovmf").context("Could not create ovmf directory")?;
    }

    if pure_path.exists() {
        println!("Using local OVFM");

        qemu_args.push("-drive".to_string());
        qemu_args.push(format!("if=pflash,format=raw,file={}", PURE_EFI_PATH));
    } else if let Ok(path) = env::var("OVMF") {
        println!("Using env OVMF");

        // QEMU will make changes to this file, so we need a local copy. We do
        // not want to overwrite it every build
        if !local_vars.exists() {
            let mut path = PathBuf::from(&path);
            path.push("OVMF_VARS.fd");

            println!("{:?}", path);

            copy(path, "ovmf/VARS.fd").context("Could not copy VARS.fd into local directory")?;
        }

        qemu_args.push("-drive".to_string());
        qemu_args.push(format!(
            "if=pflash,format=raw,readonly=on,file={}/OVMF_CODE.fd",
            path
        ));

        qemu_args.push("-drive".to_string());
        qemu_args.push("if=pflash,format=raw,file=ovmf/VARS.fd".to_string());
    } else if system_code.exists() && system_vars.exists() {
        println!("Using system OVFM");

        // QEMU will make changes to this file, so we need a local copy. We do
        // not want to overwrite it every build
        if !local_vars.exists() {
            copy(system_vars, "ovmf/VARS.fd")
                .context("Could not copy VARS.fd into local directory")?;
        }

        // For some unknown reason, system OVMF doesn't work unless KVM is
        // enabled
        if !has_kvm() {
            return Err(QEMUErrors::MissingKVM.into());
        }

        qemu_args.push("-drive".to_string());
        qemu_args.push(format!(
            "if=pflash,format=raw,readonly=on,file={}",
            SYSTEM_EFI_CODE
        ));

        qemu_args.push("-drive".to_string());
        qemu_args.push("if=pflash,format=raw,file=ovmf/VARS.fd".to_string());
    } else {
        return Err(QEMUErrors::NoOVMF.into());
    }

    qemu_args.append(&mut vec![
        "-drive".to_string(),
        "format=raw,file=fat:rw:fioxa".to_string(),
        "-drive".to_string(),
        "format=raw,file=fat:rw:src".to_string(),
    ]);

    Command::new("qemu-system-x86_64")
        .args(qemu_args)
        .spawn()
        .context("Failed to run qemu-system-x86_64")?
        .wait()
        .unwrap();

    Ok(())
}

fn clean(name: &str) -> Result<()> {
    // Build subprocess
    let mut cargo = Command::new("cargo")
        .current_dir(format!("../{}", name))
        .args(["clean"])
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to clean")?;

    if cargo.wait()?.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to clean"))
    }
}

fn build(name: &str, release: bool) -> Result<Utf8PathBuf> {
    let mut args = vec!["build", "--message-format=json-render-diagnostics"];
    if release {
        args.push("--release");
    }

    // Build subprocess
    let mut cargo = Command::new("cargo")
        .current_dir(format!("../{}", name))
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to builds")?;

    // Grab stout and read it
    let reader = BufReader::new(cargo.stdout.take().ok_or(BuildErrors::NoOutput)?);
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
                    let exec = path.ok_or(BuildErrors::MissingExec)?;
                    return Ok(exec);

                // Didn't build correctly :(
                } else {
                    return Err(BuildErrors::BuildFailed.into());
                }
            }
            // Ignore other messages
            _ => {}
        }
    }

    Err(BuildErrors::Incomplete.into())
}

/// Checks `/dev/kvm` to determine if the OS has kvm or not
#[cfg(target_os = "linux")]
fn has_kvm() -> bool {
    Path::new("/dev/kvm").exists()
}

#[cfg(not(target_os = "linux"))]
fn has_kvm() -> bool {
    // QEMU does not have KVM support on windows
    false
}
