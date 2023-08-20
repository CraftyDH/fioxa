#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct PID(pub u64);

impl From<PID> for u64 {
    fn from(val: PID) -> Self {
        val.0
    }
}

impl From<u64> for PID {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct TID(pub u64);

impl From<TID> for u64 {
    fn from(val: TID) -> Self {
        val.0
    }
}

impl From<u64> for TID {
    fn from(value: u64) -> Self {
        TID(value)
    }
}
