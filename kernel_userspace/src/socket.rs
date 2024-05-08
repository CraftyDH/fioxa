use core::mem::MaybeUninit;
use num_derive::{FromPrimitive, ToPrimitive};

use crate::{
    event::receive_event,
    make_syscall,
    object::{KernelObjectType, KernelReference, KernelReferenceID},
};

#[derive(FromPrimitive, ToPrimitive)]
pub enum SocketOperation {
    Listen,
    Connect,
    Accept,
    GetSocketListenEvent,
    Create,
    GetSocketEvent,
    Send,
    Recv,
}

#[derive(FromPrimitive, ToPrimitive)]
pub enum SocketEvents {
    RecvBufferEmpty,
    SendBufferFull,
    OtherSideClosed,
}

#[repr(C)]
pub struct MakeSocket {
    pub ltr_capacity: usize,
    pub rtl_capacity: usize,
    pub left: MaybeUninit<KernelReferenceID>,
    pub right: MaybeUninit<KernelReferenceID>,
}

#[repr(C)]
pub struct SocketRecv {
    pub socket: KernelReferenceID,
    pub result: Option<KernelReferenceID>,
    pub eof: bool,
    pub result_type: MaybeUninit<KernelObjectType>,
}

pub fn socket_create(
    ltr_capacity: usize,
    rtl_capacity: usize,
) -> (KernelReferenceID, KernelReferenceID) {
    unsafe {
        let mut sock = MakeSocket {
            ltr_capacity,
            rtl_capacity,
            left: MaybeUninit::uninit(),
            right: MaybeUninit::uninit(),
        };
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::Create as usize,
            &mut sock
        );
        (sock.left.assume_init(), sock.right.assume_init())
    }
}

pub fn socket_listen(name: &str) -> Option<KernelReferenceID> {
    unsafe {
        let result: usize;
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::Listen as usize,
            name.as_ptr(),
            name.len() => result
        );
        KernelReferenceID::from_usize(result)
    }
}

pub fn socket_connect(name: &str) -> Option<KernelReferenceID> {
    unsafe {
        let result: usize;
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::Connect as usize,
            name.as_ptr(),
            name.len() => result
        );
        KernelReferenceID::from_usize(result)
    }
}

pub fn socket_handle_get_event(handle: KernelReferenceID, ev: SocketEvents) -> KernelReferenceID {
    unsafe {
        let event_id: usize;
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::GetSocketEvent as usize,
            handle.0.get(),
            ev as usize => event_id
        );
        KernelReferenceID::from_usize(event_id).unwrap()
    }
}

pub fn socket_listen_get_event(handle: KernelReferenceID) -> KernelReferenceID {
    unsafe {
        let event_id: usize;
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::GetSocketListenEvent as usize,
            handle.0.get() => event_id
        );
        KernelReferenceID::from_usize(event_id).unwrap()
    }
}

#[derive(Debug)]
pub enum SocketRecieveResult {
    None,
    EOF,
}

pub fn socket_recv(
    handle: KernelReferenceID,
) -> Result<(KernelReferenceID, KernelObjectType), SocketRecieveResult> {
    unsafe {
        let mut recv = SocketRecv {
            socket: handle,
            result: None,
            eof: false,
            result_type: MaybeUninit::uninit(),
        };
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::Recv as usize,
            &mut recv
        );
        recv.result
            .map(|result| (result, recv.result_type.assume_init()))
            .ok_or(if recv.eof {
                SocketRecieveResult::EOF
            } else {
                SocketRecieveResult::None
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketSendResult {
    Full = 1,
    Closed,
}

pub fn socket_send(
    handle: KernelReferenceID,
    message: KernelReferenceID,
) -> Result<(), SocketSendResult> {
    unsafe {
        let result: usize;
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::Send as usize,
            handle.0.get(),
            message.0.get() => result
        );
        match result {
            0 => Ok(()),
            1 => Err(SocketSendResult::Full),
            2 => Err(SocketSendResult::Closed),
            _ => unreachable!(),
        }
    }
}

pub fn socket_accept(handle: KernelReferenceID) -> Option<KernelReferenceID> {
    unsafe {
        let id: usize;
        make_syscall!(
            crate::syscall::SOCKET,
            SocketOperation::Accept as usize,
            handle.0.get() => id
        );
        KernelReferenceID::from_usize(id)
    }
}

pub fn socket_blocking_read(
    handle: KernelReferenceID,
) -> Result<(KernelReferenceID, KernelObjectType), SocketRecieveResult> {
    let event = KernelReference::from_id(socket_handle_get_event(
        handle,
        SocketEvents::RecvBufferEmpty,
    ));

    loop {
        receive_event(event.id(), crate::event::ReceiveMode::LevelLow);
        match socket_recv(handle) {
            Err(SocketRecieveResult::None) => (),
            r => return r,
        }
    }
}

pub struct SocketListenHandle {
    listener: KernelReference,
    wait_event: KernelReference,
}

impl SocketListenHandle {
    pub fn listen(name: &str) -> Option<Self> {
        socket_listen(name).map(|s| Self::from_raw(KernelReference::from_id(s)))
    }

    pub fn from_raw(listener: KernelReference) -> Self {
        Self {
            wait_event: KernelReference::from_id(socket_listen_get_event(listener.id())),
            listener,
        }
    }

    pub fn try_accept(&self) -> Option<SocketHandle> {
        socket_accept(self.listener.id())
            .map(|s| SocketHandle::from_raw_socket(KernelReference::from_id(s)))
    }

    pub fn wait_event(&self) -> &KernelReference {
        &self.wait_event
    }

    pub fn blocking_accept(&self) -> SocketHandle {
        loop {
            match self.try_accept() {
                Some(r) => return r,
                None => receive_event(self.wait_event.id(), crate::event::ReceiveMode::LevelHigh),
            };
        }
    }
}

#[derive(Debug)]
pub struct SocketHandle {
    socket: KernelReference,
    read_empty_event: KernelReference,
    write_full_event: KernelReference,
}

impl SocketHandle {
    pub fn connect(name: &str) -> Option<SocketHandle> {
        socket_connect(name).map(|s| Self::from_raw_socket(KernelReference::from_id(s)))
    }

    pub fn from_raw_socket(socket: KernelReference) -> Self {
        Self {
            read_empty_event: KernelReference::from_id(socket_handle_get_event(
                socket.id(),
                SocketEvents::RecvBufferEmpty,
            )),
            write_full_event: KernelReference::from_id(socket_handle_get_event(
                socket.id(),
                SocketEvents::SendBufferFull,
            )),
            socket,
        }
    }

    pub fn try_recv(&self) -> Result<(KernelReference, KernelObjectType), SocketRecieveResult> {
        socket_recv(self.socket.id()).map(|(a, b)| (KernelReference::from_id(a), b))
    }

    pub fn blocking_recv(
        &self,
    ) -> Result<(KernelReference, KernelObjectType), SocketRecieveResult> {
        loop {
            match self.try_recv() {
                Err(SocketRecieveResult::None) => (),
                r => return r,
            }
            receive_event(
                self.read_empty_event.id(),
                crate::event::ReceiveMode::LevelLow,
            );
        }
    }

    pub fn try_send(&self, message: &KernelReference) -> Result<(), SocketSendResult> {
        socket_send(self.socket.id(), message.id())
    }

    pub fn blocking_send(&self, message: &KernelReference) -> Result<(), SocketSendResult> {
        self.blocking_send_raw(message.id())
    }

    pub fn blocking_send_raw(&self, message: KernelReferenceID) -> Result<(), SocketSendResult> {
        loop {
            match socket_send(self.socket.id(), message) {
                Err(SocketSendResult::Full) => (),
                r => return r,
            }
            receive_event(
                self.write_full_event.id(),
                crate::event::ReceiveMode::LevelLow,
            );
        }
    }
}
