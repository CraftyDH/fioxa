#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct PID(pub u64);

impl Into<u64> for PID {
    fn into(self) -> u64 {
        self.0
    }
}

impl From<u64> for PID {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct TID(pub u64);

impl Into<u64> for TID {
    fn into(self) -> u64 {
        self.0
    }
}

impl From<u64> for TID {
    fn from(value: u64) -> Self {
        TID(value)
    }
}
