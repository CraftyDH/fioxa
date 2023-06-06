use thiserror::Error;

#[derive(Error, Debug)]
pub enum BuildErrors {
    #[error("cargo did not output anything to stdout")]
    NoOutput,
    #[error("could not find executable")]
    MissingExec,
    #[error("build failed")]
    BuildFailed,
    #[error("build did not complete")]
    Incomplete,
}

#[derive(Debug, Error)]
pub enum QEMUErrors {
    #[error("It looks like you don't have KVM enabled. System OVFM only seems to work with KVM enabled. If you think you know what you are doing, comment out this error code & rebuild.")]
    MissingKVM,
}
