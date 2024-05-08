use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::message::MessageHandle;

pub fn deserialize<'a, T: Deserialize<'a>>(buffer: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(buffer)
}

pub fn make_message<T: Serialize>(msg: &T, buffer: &mut Vec<u8>) -> MessageHandle {
    let size =
        postcard::serialize_with_flavor(msg, postcard::ser_flavors::Size::default()).unwrap();
    unsafe {
        buffer.reserve(size);
        buffer.set_len(size);
    }
    let data = postcard::to_slice(msg, buffer).unwrap();
    MessageHandle::create(data)
}

pub fn make_message_new<T: Serialize>(msg: &T) -> MessageHandle {
    let data = postcard::to_allocvec(msg).unwrap();
    MessageHandle::create(&data)
}
