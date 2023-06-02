use core::mem::size_of;

#[repr(u64)]
#[derive(Debug, Clone, Copy)]
pub enum StreamMessageType {
    InlineData,
}

#[derive(Debug, Clone)]
pub struct StreamMessage {
    pub stream_id: u64,
    pub message_type: StreamMessageType,
    pub timestamp: u64,
    pub data: [u8; 16],
}

impl StreamMessage {
    pub fn new(ty: StreamMessageType) -> Self {
        Self {
            stream_id: 0,
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
