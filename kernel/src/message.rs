use alloc::boxed::Box;

#[derive(Debug)]
pub struct KMessage {
    pub data: Box<[u8]>,
}
