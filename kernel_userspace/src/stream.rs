use core::mem::size_of;

#[repr(u64)]
#[derive(Debug, Clone, Copy)]
pub enum StreamMessageType {
    InlineData,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamMessage {
    pub message_type: StreamMessageType,
    pub timestamp: u64,
    pub data: [u8; 16],
}

impl StreamMessage {
    pub fn new(ty: StreamMessageType) -> Self {
        Self {
            message_type: ty,
            timestamp: 0,
            data: [0; 16],
        }
    }

    pub fn write_data<T>(&mut self, data: T) {
        assert!(size_of::<T>() <= 16);

        unsafe { (self.data.as_mut_ptr() as *mut T).write(data) };
    }

    pub fn read_data<T>(&self) -> T {
        assert!(size_of::<T>() <= 16);

        unsafe { (self.data.as_ptr() as *const T).read() }
    }
}
