use num_derive::{FromPrimitive, ToPrimitive};

use crate::{
    make_syscall,
    object::{KernelReferenceID, ObjectSignal},
};

#[derive(FromPrimitive, ToPrimitive)]
pub enum PortSyscall {
    Create,
    Wait,
    Push,
}

pub fn port_create() -> KernelReferenceID {
    unsafe {
        let id: usize;
        make_syscall!(crate::syscall::PORT, PortSyscall::Create as usize => id);
        KernelReferenceID::from_usize(id).unwrap()
    }
}

#[repr(C)]
pub struct PortNotification {
    pub key: u64,
    pub ty: PortNotificationType,
}

#[repr(C)]
pub enum PortNotificationType {
    SignalOne {
        trigger: ObjectSignal,
        signals: ObjectSignal,
    },
    Interrupt {
        timestamp: u64,
    },
    User([u8; 8]),
}

pub fn port_wait(handle: KernelReferenceID, notification: &mut PortNotification) {
    unsafe {
        make_syscall!(
            crate::syscall::PORT,
            PortSyscall::Wait as usize,
            handle.0.get(),
            notification
        );
    }
}

pub fn port_wait_rs(handle: KernelReferenceID) -> PortNotification {
    let mut notif = PortNotification {
        key: Default::default(),
        ty: PortNotificationType::User(Default::default()),
    };

    port_wait(handle, &mut notif);

    notif
}

pub fn port_push(handle: KernelReferenceID, packet: &PortNotification) {
    unsafe {
        make_syscall!(
            crate::syscall::PORT,
            PortSyscall::Push as usize,
            handle.0.get(),
            packet
        );
    }
}
