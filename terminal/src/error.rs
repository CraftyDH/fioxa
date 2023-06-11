//! This is my own little version of anyhow, but with a lot more control and
//! less std dependencies

use core::{
    error::Error,
    fmt::{Debug, Display, Write},
};

extern crate alloc;
use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};

pub type Result<T, E = InternalError> = core::result::Result<T, E>;

pub trait Context<T> {
    fn context<C>(self, context: C) -> Result<T, InternalError>
    where
        C: Display;

    fn with_context<C, F>(self, f: F) -> Result<T, InternalError>
    where
        C: Display,
        F: FnOnce() -> C;
}

impl<T> Context<T> for Result<T, InternalError> {
    fn context<C>(self, context: C) -> Result<T, InternalError>
    where
        C: Display,
    {
        self.map_err(|e| e.push_context(context.to_string()))
    }

    fn with_context<C, F>(self, f: F) -> Result<T, InternalError>
    where
        C: Display,
        F: FnOnce() -> C,
    {
        self.map_err(|e| e.push_context(f().to_string()))
    }
}

/// This is an error that can store a bunch of extra context information
pub struct InternalError {
    primary: Box<dyn Error>,
    context: Vec<String>,
}

impl InternalError {
    pub fn new(primary: Box<dyn Error>, context: Vec<String>) -> Self {
        Self { primary, context }
    }

    pub fn push_context(mut self, context: String) -> Self {
        self.context.push(context);
        self
    }
}

impl Debug for InternalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        (self as &dyn Display).fmt(f)
    }
}

impl Display for InternalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Error: ")?;
        f.write_str(&self.primary.to_string())?;

        if self.context.len() != 0 {
            f.write_str("\n\nCaused by:")?;
            for ctx in self.context.iter() {
                f.write_str("\n\t")?;
                f.write_str(&ctx)?;
            }
        }

        Ok(())
    }
}

impl<E> From<E> for InternalError
where
    E: Error + Sized + 'static,
{
    fn from(value: E) -> Self {
        InternalError::new(Box::new(value), Vec::new())
    }
}
