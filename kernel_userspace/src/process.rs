use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;

use crate::{
    event::receive_event,
    make_syscall,
    object::{KernelReference, KernelReferenceID},
};

#[derive(FromPrimitive, ToPrimitive)]
pub enum KernelProcessOperation {
    GetExitCode,
    GetExitEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive, ToPrimitive)]
pub enum ProcessExit {
    Exited,
    NotExitedYet,
}

pub fn process_get_exit_code(handle: KernelReferenceID) -> ProcessExit {
    let res: usize;
    unsafe {
        make_syscall!(
            crate::syscall::PROCESS,
            KernelProcessOperation::GetExitCode as usize,
            handle.0.get() => res
        );
        ProcessExit::from_usize(res).unwrap()
    }
}

pub fn process_get_exit_event(handle: KernelReferenceID) -> KernelReferenceID {
    unsafe {
        let id: usize;
        make_syscall!(
            crate::syscall::PROCESS,
            KernelProcessOperation::GetExitEvent as usize,
            handle.0.get() => id
        );
        KernelReferenceID::from_usize(id).unwrap()
    }
}

pub struct ProcessHandle {
    handle: KernelReference,
    exit_signal: Option<KernelReference>,
}

impl ProcessHandle {
    pub fn from_kref(kref: KernelReference) -> Self {
        Self {
            handle: kref,
            exit_signal: None,
        }
    }

    pub fn get_exit_code(&self) -> ProcessExit {
        process_get_exit_code(self.handle.id())
    }

    pub fn get_exit_signal(&mut self) -> &KernelReference {
        match &mut self.exit_signal {
            Some(s) => s,
            a @ None => {
                *a = Some(KernelReference::from_id(process_get_exit_event(
                    self.handle.id(),
                )));
                a.as_ref().unwrap()
            }
        }
    }

    pub fn blocking_exit_code(&mut self) -> ProcessExit {
        loop {
            match process_get_exit_code(self.handle.id()) {
                ProcessExit::NotExitedYet => (),
                a => return a,
            };
            receive_event(
                self.get_exit_signal().id(),
                crate::event::ReceiveMode::LevelHigh,
            );
        }
    }
}
