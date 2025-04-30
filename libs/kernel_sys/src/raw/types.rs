#![allow(non_camel_case_types)]

pub type pid_t = u64;
pub type tid_t = u64;
pub type hid_t = usize;
pub type result_t = usize;
pub type vaddr_t = *mut ();
pub type signals_t = u64;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct sys_port_notification_t {
    pub key: u64,
    pub ty: u8,
    pub value: sys_port_notification_value_t,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub union sys_port_notification_value_t {
    pub one: sys_port_notification_one_t,
    pub interrupt: u64,
    pub user: [u8; 8],
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct sys_port_notification_one_t {
    pub trigger: signals_t,
    pub signals: signals_t,
}
