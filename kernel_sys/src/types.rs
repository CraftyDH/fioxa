use core::num::NonZero;

use num_derive::{FromPrimitive, ToPrimitive};
use num_traits::FromPrimitive;

use super::raw::types::*;

#[derive(Debug, Clone, Copy)]
pub enum TryFromRawError {
    InvalidValue,
}

pub trait RawValue
where
    Self: Sized,
{
    type Raw;

    fn into_raw(&self) -> Self::Raw;
    fn from_raw(raw: Self::Raw) -> Result<Self, TryFromRawError>;
}

macro_rules! non_zero_wrap {
    ($ty:ty, $raw:ty) => {
        impl RawValue for $ty {
            type Raw = $raw;

            fn into_raw(&self) -> Self::Raw {
                self.0.get()
            }

            fn from_raw(raw: Self::Raw) -> Result<Self, TryFromRawError> {
                NonZero::new(raw)
                    .map(Self)
                    .ok_or(TryFromRawError::InvalidValue)
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Pid(NonZero<pid_t>);
non_zero_wrap!(Pid, pid_t);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Tid(NonZero<tid_t>);
non_zero_wrap!(Tid, tid_t);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Hid(pub NonZero<hid_t>);
non_zero_wrap!(Hid, hid_t);

impl Hid {
    pub const fn from_usize(val: usize) -> Option<Hid> {
        match NonZero::new(val) {
            Some(v) => Some(Hid(v)),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive, ToPrimitive)]
#[must_use]
#[repr(C)]
pub enum SyscallResult {
    Ok,

    BadInputPointer,
    SystemError,
    UnknownHandle,

    ChannelEmpty,
    ChannelFull,
    ChannelClosed,
    ChannelBufferTooSmall,
    ChannelMsgTooBig,

    ProcessStillRunning,
}

impl SyscallResult {
    #[inline]
    #[track_caller]
    pub fn assert_ok(self) {
        match self {
            Self::Ok => (),
            _ => panic!("Was not ok: {self:?}"),
        }
    }

    #[inline]
    pub fn into_err(self) -> Result<(), SyscallResult> {
        match self {
            Self::Ok => Ok(()),
            v => Err(v),
        }
    }

    #[inline]
    pub fn create(val: result_t) -> Result<(), SyscallResult> {
        match Self::from_raw(val).unwrap() {
            Self::Ok => Ok(()),
            v => Err(v),
        }
    }
}

impl RawValue for SyscallResult {
    type Raw = result_t;

    #[inline]
    fn into_raw(&self) -> Self::Raw {
        *self as result_t
    }

    #[inline]
    fn from_raw(raw: Self::Raw) -> Result<Self, TryFromRawError> {
        FromPrimitive::from_usize(raw).ok_or(TryFromRawError::InvalidValue)
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct MapMemoryFlags: u32 {
        const WRITEABLE     = 1 << 0;

        const PREALLOC      = 1 << 1;
        const ALLOC_32BITS  = 1 << 2;
    }
}

#[derive(Debug, FromPrimitive, ToPrimitive, Clone, Copy, PartialEq, Eq)]
pub enum KernelObjectType {
    None,
    Message,
    Process,
    Channel,
    Port,
    Interrupt,
}

bitflags::bitflags! {
    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub struct ObjectSignal: u64 {
        const READABLE = 1 << 1;

        const CHANNEL_CLOSED = 1 << 20;

        const PROCESS_EXITED = 1 << 20;
    }
}

#[repr(C)]
pub struct SysPortNotification {
    pub key: u64,
    pub value: SysPortNotificationValue,
}

#[repr(C)]
pub enum SysPortNotificationValue {
    SignalOne {
        trigger: ObjectSignal,
        signals: ObjectSignal,
    },
    Interrupt {
        timestamp: u64,
    },
    User([u8; 8]),
}

impl RawValue for SysPortNotification {
    type Raw = sys_port_notification_t;

    #[inline]
    fn into_raw(&self) -> Self::Raw {
        let (ty, value) = match self.value {
            SysPortNotificationValue::SignalOne { trigger, signals } => {
                (0, sys_port_notification_value_t {
                    one: sys_port_notification_one_t {
                        trigger: trigger.bits(),
                        signals: signals.bits(),
                    },
                })
            }
            SysPortNotificationValue::Interrupt { timestamp } => {
                (1, sys_port_notification_value_t {
                    interrupt: timestamp,
                })
            }
            SysPortNotificationValue::User(user) => (2, sys_port_notification_value_t { user }),
        };

        sys_port_notification_t {
            key: self.key,
            ty,
            value,
        }
    }

    #[inline]
    fn from_raw(raw: Self::Raw) -> Result<Self, TryFromRawError> {
        unsafe {
            let value = match raw.ty {
                0 => SysPortNotificationValue::SignalOne {
                    trigger: ObjectSignal::from_bits_retain(raw.value.one.trigger),
                    signals: ObjectSignal::from_bits_retain(raw.value.one.signals),
                },
                1 => SysPortNotificationValue::Interrupt {
                    timestamp: raw.value.interrupt,
                },
                2 => SysPortNotificationValue::User(raw.value.user),
                _ => return Err(TryFromRawError::InvalidValue),
            };

            Ok(Self {
                key: raw.key,
                value,
            })
        }
    }
}
