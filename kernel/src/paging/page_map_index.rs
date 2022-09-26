pub struct PageMapIndexer {
    pub pdp_i: u64,
    pub pd_i: u64,
    pub pt_i: u64,
    pub p_i: u64,
}

impl PageMapIndexer {
    pub fn new(mut virtual_address: u64) -> Self {
        virtual_address >>= 12;
        let p_i = virtual_address & 0x1ff;
        virtual_address >>= 9;
        let pt_i = virtual_address & 0x1ff;
        virtual_address >>= 9;
        let pd_i = virtual_address & 0x1ff;
        virtual_address >>= 9;
        let pdp_i = virtual_address & 0x1ff;

        Self {
            pdp_i,
            pd_i,
            pt_i,
            p_i,
        }
    }
}
