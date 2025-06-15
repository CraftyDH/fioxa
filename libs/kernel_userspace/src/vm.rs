bitflags::bitflags! {
    pub struct VMOAnonymousFlags: u32 {
        // Once allocated, the physical address will stay the same (and will always be allocated)
        const PINNED = 1 << 1;

        // All physical addresses will be 32 bits
        const BELOW_32 = 1 << 2;

        // All physical address will be continuous
        const CONTINUOUS = 1 << 3;
    }
}
